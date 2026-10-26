//! One error type for every handler: renders `{"error":{"code","message"}}`.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use utoipa::ToSchema;

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Unauthorized(&'static str),
    Forbidden(&'static str),
    NotFound(&'static str),
    Conflict(&'static str),
    Internal,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorDetail {
    pub code: &'static str,
    pub message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", m),
            Self::Unauthorized(m) => (StatusCode::UNAUTHORIZED, "unauthorized", m.into()),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, "forbidden", m.into()),
            Self::NotFound(m) => (StatusCode::NOT_FOUND, "not_found", m.into()),
            Self::Conflict(m) => (StatusCode::CONFLICT, "conflict", m.into()),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal error".into(),
            ),
        };
        let body = ErrorBody {
            error: ErrorDetail { code, message },
        };
        (status, Json(body)).into_response()
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        if let sqlx::Error::Database(db) = &e
            && (db.is_unique_violation() || db.is_foreign_key_violation())
        {
            return Self::Conflict("conflicts with existing data");
        }
        tracing::error!(error = %e, "database error");
        Self::Internal
    }
}

impl From<JsonRejection> for AppError {
    fn from(e: JsonRejection) -> Self {
        Self::BadRequest(e.body_text())
    }
}

/// `Json<T>` whose parse failures render as `AppError`.
pub struct AppJson<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for AppJson<T> {
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, AppError> {
        Ok(Self(Json::<T>::from_request(req, state).await?.0))
    }
}
