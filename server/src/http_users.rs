//! Admin user list, the new-user screen, both password calls, and web onboarding.

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde_json::Value;

use crate::db::App;
use crate::http::{db_error, json_ok, message, require_session, user_json};
use crate::users::UserFail;

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/admin/users", get(search_users).post(create_user))
        .route("/api/admin/users/{id}/statistics", get(statistics))
        .route("/api/admin/users/{id}/sessions", get(sessions))
        .route("/api/admin/users/{id}/preferences", get(user_preferences))
        .route("/api/admin/users/{id}/calendar-heatmap", get(heatmap))
        .route("/api/admin/users/{id}", get(user_by_id))
        .route("/api/auth/change-password", post(change_password))
        .route("/api/users/me/onboarding", put(set_onboarding))
        .route("/api/users/{id}/profile-image", get(profile_image))
        .route("/api/public/config", get(public_config))
        .route("/api/server/storage", get(storage))
        .route(
            "/api/system-metadata/admin-onboarding",
            post(admin_onboarding),
        )
}

pub(crate) async fn update_me(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.set_my_password(&session.user.id, &body).await {
        Ok(user) => json_ok(user_json(&user)),
        Err(error) => user_error(error),
    }
}

pub(crate) async fn update_preferences(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.update_preferences(&session.user.id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => user_error(error),
    }
}

async fn set_onboarding(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.set_onboarding(&session.user.id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => user_error(error),
    }
}

async fn change_password(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app
        .change_password(&session.user.id, &session.session_id, &body)
        .await
    {
        Ok(user) => json_ok(user_json(&user)),
        Err(error) => user_error(error),
    }
}

async fn create_user(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.create_user(&body).await {
        Ok(user) => (StatusCode::CREATED, axum::Json(user_json(&user))).into_response(),
        Err(error) => user_error(error),
    }
}

async fn search_users(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    let pairs = query_pairs(&query);
    let with_deleted = match flag(pair(&pairs, "withDeleted").as_deref()) {
        Ok(value) => value,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    let id = match optional_uuid(pair(&pairs, "id").as_deref()) {
        Ok(id) => id,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.search_users(with_deleted, id.as_deref()).await {
        Ok(users) => json_ok(Value::Array(users.iter().map(user_json).collect())),
        Err(error) => db_error(error),
    }
}

async fn user_by_id(
    State(app): State<App>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.read_user(&id, true).await {
        Ok(Some(user)) => json_ok(user_json(&user)),
        Ok(None) => message(StatusCode::BAD_REQUEST, "User not found"),
        Err(error) => db_error(error),
    }
}

async fn user_preferences(
    State(app): State<App>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.read_user(&id, true).await {
        Ok(None) => return message(StatusCode::BAD_REQUEST, "User not found"),
        Err(error) => return db_error(error),
        Ok(Some(_)) => {}
    }
    match app.preferences_for(&id).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn statistics(
    State(app): State<App>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.user_statistics(&id).await {
        Ok(body) => json_ok(body),
        Err(error) => user_error(error),
    }
}

async fn sessions(State(app): State<App>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.user_sessions(&id).await {
        Ok(body) => json_ok(body),
        Err(error) => user_error(error),
    }
}

async fn heatmap(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let pairs = query_pairs(&query);
    let from = pair(&pairs, "from");
    let to = pair(&pairs, "to");
    let kind = pair(&pairs, "type").unwrap_or_else(|| "Upload".to_string());
    match app
        .user_heatmap(&id, from.as_deref(), to.as_deref(), &kind)
        .await
    {
        Ok(body) => json_ok(body),
        Err(error) => user_error(error),
    }
}

async fn profile_image(
    State(app): State<App>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_session(&app, &headers).await {
        return response;
    }
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.profile_image(&id).await {
        Ok((content_type, bytes)) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type)],
            bytes,
        )
            .into_response(),
        Err(error) => user_error(error),
    }
}

async fn public_config(State(app): State<App>) -> Response {
    let config = match app.admin_config().await {
        Ok(config) => config,
        Err(error) => return db_error(error),
    };
    json_ok(app.public_config_body(&config))
}

async fn storage(State(app): State<App>, headers: HeaderMap) -> Response {
    if let Err(response) = require_session(&app, &headers).await {
        return response;
    }
    json_ok(app.storage_report())
}

async fn admin_onboarding(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = require_admin(&app, &headers).await {
        return response;
    }
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.set_admin_onboarding(&body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => user_error(error),
    }
}

async fn require_admin(app: &App, headers: &HeaderMap) -> Result<(), Response> {
    let session = require_session(app, headers).await?;
    if session.user.is_admin {
        Ok(())
    } else {
        Err(message(StatusCode::FORBIDDEN, "Forbidden"))
    }
}

fn user_error(error: UserFail) -> Response {
    match error {
        UserFail::Db(error) => db_error(error),
        UserFail::BadRequest(text) => message(StatusCode::BAD_REQUEST, text),
        UserFail::NotFound(text) => message(StatusCode::NOT_FOUND, text),
    }
}

async fn read_json(request: Request) -> Result<Value, &'static str> {
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024 * 1024).await else {
        return Err("Validation failed");
    };
    serde_json::from_slice(&bytes).map_err(|_| "Validation failed")
}

fn flag(value: Option<&str>) -> Result<bool, &'static str> {
    match value {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err("Validation failed"),
    }
}

fn optional_uuid(value: Option<&str>) -> Result<Option<String>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if uuid::Uuid::parse_str(value).is_err() {
        return Err("Validation failed");
    }
    Ok(Some(value.to_string()))
}

fn query_pairs(query: &str) -> Vec<(String, String)> {
    if query.is_empty() {
        return Vec::new();
    }
    query
        .split('&')
        .map(|item| {
            let (name, value) = item.split_once('=').unwrap_or((item, ""));
            (decode(name), decode(value))
        })
        .collect()
}

fn pair(pairs: &[(String, String)], name: &str) -> Option<String> {
    pairs
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

fn decode(value: &str) -> String {
    let mut output = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                output.push(byte);
                index += 3;
                continue;
            }
        }
        if bytes[index] == b'+' {
            output.push(b' ');
        } else {
            output.push(bytes[index]);
        }
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}
