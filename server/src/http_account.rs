//! Account settings the signed-in web UI calls: API keys, sessions, profile image,
//! notifications, usage counts, and the caller heatmap.

use std::path::PathBuf;

use axum::extract::{Multipart, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::account::{holds, AccountFail};
use crate::db::{new_id, App, DbError};
use crate::http::{cookie, db_error, json_ok, message, require_session};
use crate::http_shares::{library_auth, share_params, ShareQuery};
use crate::users::UserFail;

pub(crate) struct Caller {
    pub user_id: String,
    pub session_id: Option<String>,
    pub permissions: Option<Vec<String>>,
}

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/api-keys", get(list_api_keys).post(create_api_key))
        .route(
            "/api/api-keys/{id}",
            put(update_api_key).delete(delete_api_key),
        )
        .route("/api/api-keys/{id}/rotate", post(rotate_api_key))
        .route("/api/sessions", get(list_sessions).delete(delete_sessions))
        .route("/api/sessions/{id}", delete(delete_session))
        .route("/api/users/profile-image", post(upload_profile))
        .route(
            "/api/notifications",
            get(list_notifications).put(update_notifications),
        )
        .route("/api/notifications/{id}", put(update_notification))
        .route("/api/assets/statistics", get(asset_statistics))
        .route("/api/users/me/calendar-heatmap", get(my_heatmap))
}

async fn list_api_keys(State(app): State<App>, request: Request) -> Response {
    let (headers, query) = request_parts(&request);
    let caller = match account_caller(&app, &headers, &query, "apiKey.read").await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    match app.list_api_keys(&caller.user_id).await {
        Ok(body) => json_ok(body),
        Err(error) => account_error(error),
    }
}

async fn create_api_key(State(app): State<App>, request: Request) -> Response {
    let (caller, body) = match caller_json(&app, request, "apiKey.create").await {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    match app
        .create_api_key(&caller.user_id, caller.permissions.as_deref(), &body)
        .await
    {
        Ok(body) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Err(error) => account_error(error),
    }
}

async fn update_api_key(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (caller, body) = match caller_json(&app, request, "apiKey.update").await {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app
        .update_api_key(&caller.user_id, &id, caller.permissions.as_deref(), &body)
        .await
    {
        Ok(body) => json_ok(body),
        Err(error) => account_error(error),
    }
}

async fn rotate_api_key(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (headers, query) = request_parts(&request);
    let caller = match account_caller(&app, &headers, &query, "apiKey.rotate").await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app
        .rotate_api_key(&caller.user_id, &id, caller.permissions.as_deref())
        .await
    {
        Ok(body) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Err(error) => account_error(error),
    }
}

async fn delete_api_key(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (headers, query) = request_parts(&request);
    let caller = match account_caller(&app, &headers, &query, "apiKey.delete").await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.delete_api_key(&caller.user_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => account_error(error),
    }
}

async fn list_sessions(State(app): State<App>, request: Request) -> Response {
    let (headers, query) = request_parts(&request);
    let caller = match account_caller(&app, &headers, &query, "session.read").await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    match app
        .my_sessions(&caller.user_id, caller.session_id.as_deref())
        .await
    {
        Ok(body) => json_ok(body),
        Err(error) => account_error(error),
    }
}

async fn delete_sessions(State(app): State<App>, request: Request) -> Response {
    let (headers, query) = request_parts(&request);
    let caller = match account_caller(&app, &headers, &query, "session.delete").await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    match app
        .delete_other_sessions(&caller.user_id, caller.session_id.as_deref())
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => account_error(error),
    }
}

async fn delete_session(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (headers, query) = request_parts(&request);
    let caller = match account_caller(&app, &headers, &query, "session.delete").await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.delete_my_session(&caller.user_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => account_error(error),
    }
}

#[derive(Deserialize, Default)]
struct AuthQuery {
    key: Option<String>,
    slug: Option<String>,
    #[serde(rename = "apiKey")]
    api_key: Option<String>,
}

async fn upload_profile(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<AuthQuery>,
    mut multipart: Multipart,
) -> Response {
    let mut raw = Vec::new();
    if let Some(key) = &query.key {
        raw.push(format!("key={key}"));
    }
    if let Some(slug) = &query.slug {
        raw.push(format!("slug={slug}"));
    }
    if let Some(api_key) = &query.api_key {
        raw.push(format!("apiKey={api_key}"));
    }
    let caller =
        match account_caller(&app, &headers, &raw.join("&"), "userProfileImage.update").await {
            Ok(caller) => caller,
            Err(response) => return response,
        };
    let tmp = app.data_dir().join("tmp");
    if let Err(error) = tokio::fs::create_dir_all(&tmp).await {
        return db_error(DbError::Store(error.to_string()));
    }
    let mut file: Option<PathBuf> = None;
    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => {
                remove_temp(file.as_deref()).await;
                return message(StatusCode::BAD_REQUEST, "Validation failed");
            }
        };
        if field.name() != Some("file") {
            continue;
        }
        let path = tmp.join(format!("profile-{}", new_id()));
        let mut output = match tokio::fs::File::create(&path).await {
            Ok(output) => output,
            Err(error) => return db_error(DbError::Store(error.to_string())),
        };
        let mut failed = false;
        while let Some(chunk) = match field.chunk().await {
            Ok(chunk) => chunk,
            Err(_) => {
                failed = true;
                None
            }
        } {
            if output.write_all(&chunk).await.is_err() {
                failed = true;
                break;
            }
        }
        if failed {
            let _ = tokio::fs::remove_file(&path).await;
            remove_temp(file.as_deref()).await;
            return message(StatusCode::BAD_REQUEST, "Validation failed");
        }
        if let Some(previous) = file.replace(path) {
            let _ = tokio::fs::remove_file(previous).await;
        }
    }
    let Some(path) = file else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    match app.set_profile_image(&caller.user_id, &path).await {
        Ok(body) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Err(error) => account_error(error),
    }
}

async fn list_notifications(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    let caller = match account_caller(&app, &headers, &query, "notification.read").await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let unread = match flag(&query, "unread") {
        Ok(unread) => unread,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.list_notifications(&caller.user_id, unread).await {
        Ok(body) => json_ok(body),
        Err(error) => account_error(error),
    }
}

async fn update_notifications(State(app): State<App>, request: Request) -> Response {
    let (caller, body) = match caller_json(&app, request, "notification.update").await {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    match app.update_notifications(&caller.user_id, &body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => account_error(error),
    }
}

async fn update_notification(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (caller, body) = match caller_json(&app, request, "notification.update").await {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.update_notification(&caller.user_id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => account_error(error),
    }
}

async fn asset_statistics(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    let caller = match account_caller(&app, &headers, &query, "asset.statistics").await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    match app.asset_statistics(&caller.user_id, &query).await {
        Ok(body) => json_ok(body),
        Err(error) => account_error(error),
    }
}

async fn my_heatmap(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    let caller = match account_caller(&app, &headers, &query, "user.read").await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let pairs = query_pairs(&query);
    let from = pair(&pairs, "from");
    let to = pair(&pairs, "to");
    let kind = pair(&pairs, "type").unwrap_or_else(|| "Upload".to_string());
    match app
        .user_heatmap(&caller.user_id, from.as_deref(), to.as_deref(), &kind)
        .await
    {
        Ok(body) => json_ok(body),
        Err(error) => user_error(error),
    }
}

fn request_parts(request: &Request) -> (HeaderMap, String) {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    (headers, query)
}

async fn caller_json(
    app: &App,
    request: Request,
    permission: &str,
) -> Result<(Caller, Value), Response> {
    let (headers, query) = request_parts(&request);
    let caller = account_caller(app, &headers, &query, permission).await?;
    let body = read_json(request).await?;
    Ok((caller, body))
}

pub(crate) async fn account_caller(
    app: &App,
    headers: &HeaderMap,
    query: &str,
    permission: &str,
) -> Result<Caller, Response> {
    let pairs = query_pairs(query);
    let share = ShareQuery {
        key: pair(&pairs, "key"),
        slug: pair(&pairs, "slug"),
        ..ShareQuery::default()
    };
    let (share_key, share_slug) = share_params(headers, &share);
    if share_key.is_some() || share_slug.is_some() {
        library_auth(app, headers, &share, false).await?;
        return Err(message(StatusCode::FORBIDDEN, "Forbidden"));
    }
    if cookie(headers, "immich_access_token").is_some() {
        let session = require_session(app, headers).await?;
        return Ok(Caller {
            user_id: session.user.id,
            session_id: Some(session.session_id),
            permissions: None,
        });
    }
    if let Some(token) = header_value(headers, "x-api-key").or_else(|| pair(&pairs, "apiKey")) {
        if token.is_empty() {
            return Err(message(StatusCode::UNAUTHORIZED, "Invalid API key"));
        }
        let found = app.lookup_api_key(&token).await.map_err(db_error)?;
        let Some(found) = found else {
            return Err(message(StatusCode::UNAUTHORIZED, "Invalid API key"));
        };
        if !holds(&found.permissions, permission) {
            return Err(message(
                StatusCode::FORBIDDEN,
                format!("Missing required permission: {permission}"),
            ));
        }
        return Ok(Caller {
            user_id: found.user_id,
            session_id: None,
            permissions: Some(found.permissions),
        });
    }
    if cookie(headers, "immich_shared_link_token").is_some() {
        return Err(message(StatusCode::FORBIDDEN, "Forbidden"));
    }
    Err(message(StatusCode::UNAUTHORIZED, "Authentication required"))
}

fn account_error(error: AccountFail) -> Response {
    match error {
        AccountFail::Db(error) => db_error(error),
        AccountFail::BadRequest(text) => message(StatusCode::BAD_REQUEST, text),
        AccountFail::Unauthorized(text) => message(StatusCode::UNAUTHORIZED, text),
    }
}

fn user_error(error: UserFail) -> Response {
    match error {
        UserFail::Db(error) => db_error(error),
        UserFail::BadRequest(text) => message(StatusCode::BAD_REQUEST, text),
        UserFail::Forbidden(text) => message(StatusCode::FORBIDDEN, text),
        UserFail::NotFound(text) => message(StatusCode::NOT_FOUND, text),
    }
}

async fn read_json(request: Request) -> Result<Value, Response> {
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024 * 1024).await else {
        return Err(message(StatusCode::BAD_REQUEST, "Validation failed"));
    };
    serde_json::from_slice(&bytes)
        .map_err(|_| message(StatusCode::BAD_REQUEST, "Validation failed"))
}

async fn remove_temp(path: Option<&std::path::Path>) {
    if let Some(path) = path {
        let _ = tokio::fs::remove_file(path).await;
    }
}

fn flag(query: &str, name: &str) -> Result<Option<bool>, &'static str> {
    match pair(&query_pairs(query), name).as_deref() {
        None => Ok(None),
        Some("true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        Some(_) => Err("Validation failed"),
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
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
