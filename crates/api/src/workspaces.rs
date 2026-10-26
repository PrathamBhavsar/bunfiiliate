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
use crate::auth::{Admin, Auth, Principal, SuperAdmin, hash_password};
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
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| AppError::Internal)??;
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
