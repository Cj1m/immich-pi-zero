//! Album activity HTTP. A shared link is forbidden on every route.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::Value;
use uuid::Uuid;

use crate::activity::{ActivityError, ActivitySearch, ActivityWrite};
use crate::db::App;
use crate::http::{cookie, db_error, json_ok, message, require_session};
use crate::http_shares::{library_auth, ShareQuery};

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/activities/statistics", get(statistics))
        .route("/api/activities", get(list_activities).post(create_activity))
        .route("/api/activities/{id}", axum::routing::delete(delete_activity))
}

async fn list_activities(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match viewer(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let search = match parse_search(&raw_query) {
        Ok(search) => search,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.list_activities(&user_id, &search).await {
        Ok(rows) => json_ok(Value::Array(rows)),
        Err(error) => activity_error(error),
    }
}

async fn statistics(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match viewer(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let Some(album_id) = parse_album(&raw_query) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let asset_id = match optional_uuid(&query_pairs(&raw_query), "assetId") {
        Ok(asset_id) => asset_id,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.activity_statistics(&user_id, &album_id, asset_id.as_deref()).await {
        Ok(body) => json_ok(body),
        Err(error) => activity_error(error),
    }
}

async fn create_activity(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match viewer(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024 * 1024).await else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    match app.create_activity(&user_id, &body).await {
        Ok(ActivityWrite::Created(body)) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Ok(ActivityWrite::Existing(body)) => json_ok(body),
        Err(error) => activity_error(error),
    }
}

async fn delete_activity(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match viewer(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.delete_activity(&user_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => activity_error(error),
    }
}

async fn viewer(app: &App, headers: &HeaderMap, query: &str) -> Result<String, Response> {
    let pairs = query_pairs(query);
    let share = ShareQuery {
        key: pair(&pairs, "key"),
        slug: pair(&pairs, "slug"),
        ..ShareQuery::default()
    };
    let share_header = headers.get("x-immich-share-key").is_some() || headers.get("x-immich-share-slug").is_some();
    if share.key.is_some() || share.slug.is_some() || share_header {
        return match library_auth(app, headers, &share, false).await {
            Err(response) => Err(response),
            Ok(_) => Err(message(StatusCode::FORBIDDEN, "Forbidden")),
        };
    }
    if cookie(headers, "immich_access_token").is_none() && cookie(headers, "immich_shared_link_token").is_some() {
        return Err(message(StatusCode::FORBIDDEN, "Forbidden"));
    }
    Ok(require_session(app, headers).await?.user.id)
}

fn parse_search(query: &str) -> Result<ActivitySearch, &'static str> {
    let pairs = query_pairs(query);
    let Some(album_id) = parse_album(query) else {
        return Err("Validation failed");
    };
    let album_level = match pair(&pairs, "level").as_deref() {
        None => false,
        Some("album") => true,
        Some("asset") => false,
        Some(_) => return Err("Validation failed"),
    };
    let asset_id = if album_level {
        None
    } else {
        optional_uuid(&pairs, "assetId")?
    };
    let liked = match pair(&pairs, "type").as_deref() {
        None => None,
        Some("like") => Some(1),
        Some("comment") => Some(0),
        Some(_) => return Err("Validation failed"),
    };
    let user_id = optional_uuid(&pairs, "userId")?;
    Ok(ActivitySearch {
        album_id,
        user_id,
        asset_id,
        album_level,
        liked,
    })
}

fn parse_album(query: &str) -> Option<String> {
    let pairs = query_pairs(query);
    let album_id = pair(&pairs, "albumId")?;
    Uuid::parse_str(&album_id).ok()?;
    Some(album_id)
}

fn optional_uuid(pairs: &[(String, String)], name: &str) -> Result<Option<String>, &'static str> {
    match pair(pairs, name) {
        None => Ok(None),
        Some(id) if Uuid::parse_str(&id).is_ok() => Ok(Some(id)),
        Some(_) => Err("Validation failed"),
    }
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
    pairs.iter().rev().find(|(key, _)| key == name).map(|(_, value)| value.clone())
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

fn activity_error(error: ActivityError) -> Response {
    match error {
        ActivityError::Db(error) => db_error(error),
        ActivityError::Status(status, text) => message(status, text),
    }
}
