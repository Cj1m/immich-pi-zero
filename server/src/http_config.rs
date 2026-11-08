//! Admin config routes. A missing session is 401. A signed-in user who is not an admin is 403.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::Value;

use crate::db::App;
use crate::http::{db_error, json_ok, message, require_session};
use crate::system_config::{storage_template_options, ConfigError};

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/system-config/defaults", get(get_defaults))
        .route(
            "/api/system-config/storage-template-options",
            get(storage_options),
        )
        .route("/api/system-config", get(get_config).put(put_config))
        .route("/api/admin/config/defaults", get(get_defaults))
        .route("/api/admin/config", get(get_config).put(put_config))
        .route("/api/admin/notifications/test-email", post(test_email))
}

async fn require_admin(app: &App, headers: &HeaderMap) -> Result<(), Response> {
    let session = require_session(app, headers).await?;
    if session.user.is_admin {
        Ok(())
    } else {
        Err(message(StatusCode::FORBIDDEN, "Forbidden"))
    }
}

async fn get_config(State(app): State<App>, headers: HeaderMap) -> Response {
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    match app.admin_config().await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn get_defaults(State(app): State<App>, headers: HeaderMap) -> Response {
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    json_ok(app.config_defaults())
}

async fn storage_options(State(app): State<App>, headers: HeaderMap) -> Response {
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    json_ok(storage_template_options())
}

async fn put_config(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024 * 1024).await else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    match app.replace_admin_config(&body).await {
        Ok(body) => json_ok(body),
        Err(ConfigError::Db(error)) => db_error(error),
        Err(ConfigError::Message(text)) => message(StatusCode::BAD_REQUEST, text),
    }
}

async fn test_email(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024 * 1024).await else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    if serde_json::from_slice::<Value>(&bytes).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    message(
        StatusCode::BAD_REQUEST,
        "Failed to verify SMTP configuration",
    )
}
