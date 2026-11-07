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
    /// Missing or mismatched `X-CSRF-Token`.
    Csrf,
    NotFound(&'static str),
    Conflict(&'static str),
    /// Well-formed request the server cannot act on (e.g. a payout rail with no credentials).
    Unprocessable(&'static str),
    TooManyRequests,
    PayloadTooLarge,
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
            Self::Csrf => (
                StatusCode::FORBIDDEN,
                "csrf",
                "csrf token missing or invalid".into(),
            ),
            Self::NotFound(m) => (StatusCode::NOT_FOUND, "not_found", m.into()),
            Self::Conflict(m) => (StatusCode::CONFLICT, "conflict", m.into()),
            Self::Unprocessable(m) => (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable", m.into()),
            Self::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "request body too large".into(),
            ),
            Self::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                "too many requests, try again later".into(),
            ),
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
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            Self::PayloadTooLarge
        } else {
            Self::BadRequest(e.body_text())
        }
    }
}

/// `Json<T>` whose parse failures render as `AppError`.
pub struct AppJson<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for AppJson<T> {
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, AppError> {
        let Json(value) = Json::<serde_json::Value>::from_request(req, state).await?;
        // Postgres text and jsonb reject U+0000; refuse it here instead of failing as a 500.
        if has_nul(&value) {
            return Err(AppError::BadRequest(
                "strings must not contain NUL characters".into(),
            ));
        }
        serde_json::from_value(value)
            .map(Self)
            .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))
    }
}

fn has_nul(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::String(s) => s.contains('\0'),
        serde_json::Value::Array(a) => a.iter().any(has_nul),
        serde_json::Value::Object(m) => m.iter().any(|(k, v)| k.contains('\0') || has_nul(v)),
        _ => false,
    }
}
