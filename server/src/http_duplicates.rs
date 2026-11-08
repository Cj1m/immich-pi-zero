//! Duplicate HTTP. A share key or a shared-link cookie is 403. The access-token cookie is the session.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::Value;

use crate::db::App;
use crate::http::{cookie, db_error, json_ok, message, require_session};
use crate::http_shares::{library_auth, ShareQuery};

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route(
            "/api/duplicates",
            get(list_duplicates).delete(delete_duplicates),
        )
        .route("/api/duplicates/resolve", post(resolve_duplicates))
        .route(
            "/api/duplicates/{id}",
            axum::routing::delete(delete_duplicate),
        )
}

async fn list_duplicates(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.list_duplicates(&user_id).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn delete_duplicates(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.delete_duplicates(&user_id, &body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn delete_duplicate(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.delete_duplicate(&user_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn resolve_duplicates(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.resolve_duplicates(&user_id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

/// Session cookie, or 403 when the caller is a shared link. Checked before the body is read.
pub(crate) async fn signed_in(
    app: &App,
    headers: &HeaderMap,
    query: &str,
) -> Result<String, Response> {
    let share = ShareQuery {
        key: query_value(query, "key"),
        slug: query_value(query, "slug"),
        ..ShareQuery::default()
    };
    let share_header =
        headers.get("x-immich-share-key").is_some() || headers.get("x-immich-share-slug").is_some();
    if share.key.is_some() || share.slug.is_some() || share_header {
        return match library_auth(app, headers, &share, false).await {
            Err(response) => Err(response),
            Ok(_) => Err(message(StatusCode::FORBIDDEN, "Forbidden")),
        };
    }
    if cookie(headers, "immich_access_token").is_none()
        && cookie(headers, "immich_shared_link_token").is_some()
    {
        return Err(message(StatusCode::FORBIDDEN, "Forbidden"));
    }
    Ok(require_session(app, headers).await?.user.id)
}

fn query_value(query: &str, name: &str) -> Option<String> {
    query.split('&').rev().find_map(|item| {
        let (key, value) = item.split_once('=').unwrap_or((item, ""));
        if decode(key) == name {
            Some(decode(value))
        } else {
            None
        }
    })
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

async fn json_body(request: Request) -> Result<Value, &'static str> {
    let bytes = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .map_err(|_| "Validation failed")?;
    if bytes.is_empty() {
        return Err("Validation failed");
    }
    serde_json::from_slice(&bytes).map_err(|_| "Validation failed")
}
