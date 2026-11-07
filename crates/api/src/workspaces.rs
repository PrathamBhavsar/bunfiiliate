//! `/api/v1/workspaces` (super_admin) and `/api/v1/members` (admin).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use bf_db::auth::{User, UserRole};
use bf_db::workspaces::{Member, Workspace};
use serde::Deserialize;
use serde_json::json;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Admin, Auth, Principal, SuperAdmin, hash_password_bounded};
use crate::bootstrap::{MIN_PASSWORD_LEN, slugify};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::page::{PageQuery, Paged};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route("/api/v1/members", get(list_members).post(invite_member))
        .route("/api/v1/signup", axum::routing::post(signup))
}

/// Self-serve signup is off unless `SIGNUP_ENABLED` is `1` or `true`.
fn signup_enabled() -> bool {
    std::env::var("SIGNUP_ENABLED").is_ok_and(|v| matches!(v.trim(), "1" | "true"))
}

const MAX_SLUG_TRIES: u32 = 10;

#[derive(Deserialize, ToSchema)]
pub struct SignupRequest {
    workspace_name: String,
    admin_email: String,
    admin_password: String,
}

/// Self-serve signup: a new workspace with its first `admin`, signed in. 404 unless enabled.
#[utoipa::path(
    post,
    path = "/api/v1/signup",
    tag = "workspaces",
    request_body = SignupRequest,
    responses(
        (status = 201, description = "Workspace created, signed in", body = crate::routes::UserView),
        (status = 400, body = ErrorBody),
        (status = 404, description = "Signup disabled", body = ErrorBody),
        (status = 409, description = "Email or slug taken", body = ErrorBody),
        (status = 429, body = ErrorBody),
    )
)]
pub async fn signup(
    State(s): State<AppState>,
    headers: axum::http::HeaderMap,
    AppJson(req): AppJson<SignupRequest>,
) -> Result<
    (
        StatusCode,
        axum::http::HeaderMap,
        Json<crate::routes::UserView>,
    ),
    AppError,
> {
    if !signup_enabled() {
        return Err(AppError::NotFound("not found"));
    }
    crate::limit::check(
        &s.limiter,
        &crate::limit::SIGNUP,
        &headers,
        &req.admin_email,
    )?;
    let name = req.workspace_name.trim();
    let email = req.admin_email.trim();
    if name.is_empty() || name.len() > 200 {
        return Err(AppError::BadRequest(
            "workspace_name must be 1-200 characters".into(),
        ));
    }
    if !email.contains('@') || email.len() > 200 {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    if req.admin_password.len() < MIN_PASSWORD_LEN {
        return Err(AppError::BadRequest(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    let password_hash = hash_password_bounded(req.admin_password).await?;
    let ws = Uuid::now_v7();
    let user = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: email.to_string(),
        password_hash,
        role: UserRole::Admin,
    };
    let mut tx = s.pool.begin().await?;
    if bf_db::auth::user_by_email(&mut *tx, email).await?.is_some() {
        return Err(AppError::Conflict("email already registered"));
    }
    let base = slugify(name);
    let mut slug = None;
    for n in 1..=MAX_SLUG_TRIES {
        let cand = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        if bf_db::workspaces::workspace_id_by_slug(&mut *tx, &cand)
            .await?
            .is_none()
        {
            slug = Some(cand);
            break;
        }
    }
    let slug = slug.ok_or(AppError::Conflict("workspace name taken"))?;
    // The default-plan subscription insert lives in bf_db::billing (B09).
    bf_db::auth::create_workspace(&mut *tx, ws, name, &slug).await?;
    bf_db::workspaces::mark_self_serve(&mut *tx, ws).await?;
    bf_db::auth::create_user(&mut *tx, &user).await?;
    let principal = Principal::from(&user);
    let diff = json!({ "email": user.email, "slug": slug });
    audit::record(
        &mut *tx,
        &principal,
        "workspace.signup",
        "workspace",
        ws,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    let cookies = crate::routes::start_session(&s, &user).await?;
    Ok((StatusCode::CREATED, cookies, Json(user.into())))
}

#[derive(Deserialize, ToSchema)]
pub struct NewWorkspace {
    name: String,
    /// Defaults to a slug of `name`.
    slug: Option<String>,
}

/// Create a workspace.
#[utoipa::path(
    post,
    path = "/api/v1/workspaces",
    tag = "workspaces",
    security(("cookie" = [])),
    request_body = NewWorkspace,
    responses(
        (status = 201, body = Workspace),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 409, description = "Slug taken", body = ErrorBody),
    )
)]
pub async fn create_workspace(
    State(s): State<AppState>,
    auth: Auth<SuperAdmin>,
    AppJson(req): AppJson<NewWorkspace>,
) -> Result<(StatusCode, Json<Workspace>), AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    let slug = req.slug.as_deref().map_or_else(|| slugify(name), slugify);
    let ws = Workspace {
        id: Uuid::now_v7(),
        name: name.to_string(),
        slug,
    };
    let mut tx = s.pool.begin().await?;
    bf_db::auth::create_workspace(&mut *tx, ws.id, &ws.name, &ws.slug).await?;
    let diff = json!({ "name": ws.name, "slug": ws.slug });
    audit::record(
        &mut *tx,
        &auth.principal,
        "workspace.create",
        "workspace",
        ws.id,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(ws)))
}

/// List all workspaces.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces",
    tag = "workspaces",
    security(("cookie" = [])),
    params(PageQuery),
    responses((status = 200, body = Paged<Workspace>), (status = 403, body = ErrorBody))
)]
pub async fn list_workspaces(
    State(s): State<AppState>,
    _: Auth<SuperAdmin>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Workspace>>, AppError> {
    let rows = bf_db::workspaces::list_workspaces(&s.pool, page.limit(), page.offset()).await?;
    Ok(Json(page.wrap(rows)))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorkspaceQuery {
    /// Super admins only: act on another workspace.
    workspace_id: Option<Uuid>,
}

#[derive(Deserialize, ToSchema)]
pub struct NewMember {
    email: String,
    /// Initial password (min 8 characters); the new admin should change it.
    password: String,
}

/// Add an admin to the workspace.
#[utoipa::path(
    post,
    path = "/api/v1/members",
    tag = "members",
    security(("cookie" = [])),
    params(WorkspaceQuery),
    request_body = NewMember,
    responses(
        (status = 201, body = Member),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 409, description = "Email taken", body = ErrorBody),
    )
)]
pub async fn invite_member(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(q): Query<WorkspaceQuery>,
    AppJson(req): AppJson<NewMember>,
) -> Result<(StatusCode, Json<Member>), AppError> {
    let ws = auth.principal.workspace_scope(q.workspace_id)?;
    let email = req.email.trim();
    if !email.contains('@') {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    if req.password.len() < MIN_PASSWORD_LEN {
        return Err(AppError::BadRequest(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    let password = req.password;
    let password_hash = hash_password_bounded(password).await?;
    let user = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: email.to_string(),
        password_hash,
        role: UserRole::Admin,
    };
    let mut tx = s.pool.begin().await?;
    bf_db::auth::create_user(&mut *tx, &user).await?;
    let actor = Principal {
        workspace_id: Some(ws),
        ..auth.principal
    };
    let diff = json!({ "email": user.email, "role": "admin" });
    audit::record(
        &mut *tx,
        &actor,
        "member.invite",
        "user",
        user.id,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    let member = Member {
        id: user.id,
        email: user.email,
        role: "admin".into(),
    };
    Ok((StatusCode::CREATED, Json(member)))
}

/// List the workspace's admins.
#[utoipa::path(
    get,
    path = "/api/v1/members",
    tag = "members",
    security(("cookie" = [])),
    params(WorkspaceQuery, PageQuery),
    responses((status = 200, body = Paged<Member>), (status = 403, body = ErrorBody))
)]
pub async fn list_members(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(q): Query<WorkspaceQuery>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Member>>, AppError> {
    let ws = auth.principal.workspace_scope(q.workspace_id)?;
    let rows = bf_db::workspaces::list_admins(&s.pool, ws, page.limit(), page.offset()).await?;
    Ok(Json(page.wrap(rows)))
}
