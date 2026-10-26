//! First-run setup: one workspace and its super_admin.

use bf_db::auth::{User, UserRole};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit;
use crate::auth::{Principal, hash_password};
use crate::error::AppError;

const MIN_PASSWORD_LEN: usize = 8;

fn slugify(name: &str) -> String {
    let slug = name
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "workspace".into()
    } else {
        slug
    }
}

/// Create the workspace and super_admin user. Fails if a super_admin already exists.
pub async fn bootstrap(
    pool: &PgPool,
    email: &str,
    password: &str,
    workspace_name: &str,
) -> Result<Uuid, AppError> {
    if !email.contains('@') {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    if password.len() < MIN_PASSWORD_LEN {
        return Err(AppError::BadRequest(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    let user = User {
        id: Uuid::now_v7(),
        workspace_id: Some(Uuid::now_v7()),
        email: email.to_string(),
        password_hash: hash_password(password)?,
        role: UserRole::SuperAdmin,
    };
    let mut tx = pool.begin().await?;
    if bf_db::auth::super_admin_exists(&mut *tx).await? {
        return Err(AppError::BadRequest("a super_admin already exists".into()));
    }
    let ws = user.workspace_id.expect("set above");
    bf_db::auth::create_workspace(&mut *tx, ws, workspace_name, &slugify(workspace_name)).await?;
    bf_db::auth::create_user(&mut *tx, &user).await?;
    let diff = json!({ "email": user.email, "role": "super_admin" });
    audit::record(
        &mut *tx,
        &Principal::from(&user),
        "bootstrap",
        "user",
        user.id,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    Ok(user.id)
}
