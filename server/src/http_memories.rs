//! Memory list, save, and delete. The phone reads the sync that follows.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::Value;
use uuid::Uuid;

use crate::db::App;
use crate::http::{cookie, db_error, json_ok, message, require_session};
use crate::http_shares::{library_auth, ShareQuery};
use crate::memories::{MemoryError, MemoryOrder, MemoryQuery};

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/memories/statistics", get(statistics))
        .route("/api/memories", get(list_memories))
        .route(
            "/api/memories/{id}/assets",
            axum::routing::delete(remove_assets),
        )
        .route(
            "/api/memories/{id}",
            axum::routing::put(update_memory)
                .patch(update_memory)
                .delete(delete_memory),
        )
}

async fn list_memories(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let (owner_id, query) = match authorized(&app, &headers, &raw_query).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let memories = match app.search_memories(&owner_id, &query).await {
        Ok(memories) => memories,
        Err(error) => return memory_error(error),
    };
    json_ok(Value::Array(memories))
}

async fn statistics(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let (owner_id, query) = match authorized(&app, &headers, &raw_query).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.memory_statistics(&owner_id, &query).await {
        Ok(total) => json_ok(serde_json::json!({ "total": total })),
        Err(error) => memory_error(error),
    }
}

async fn update_memory(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    let owner_id = match memory_owner(&app, &headers, &query).await {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024 * 1024).await else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    match app.update_memory(&owner_id, &id, &body).await {
        Ok(memory) => json_ok(memory),
        Err(error) => memory_error(error),
    }
}

async fn delete_memory(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    let owner_id = match memory_owner(&app, &headers, &query).await {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match app.delete_memory(&owner_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => memory_error(error),
    }
}

async fn remove_assets(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    let owner_id = match memory_owner(&app, &headers, &query).await {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024 * 1024).await else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    let Some(ids) = body.get("ids").and_then(Value::as_array) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let mut asset_ids = Vec::new();
    for id in ids {
        let Some(id) = id.as_str() else {
            return message(StatusCode::BAD_REQUEST, "Validation failed");
        };
        asset_ids.push(id.to_string());
    }
    match app.remove_memory_assets(&owner_id, &id, &asset_ids).await {
        Ok(results) => json_ok(Value::Array(results)),
        Err(error) => memory_error(error),
    }
}

async fn authorized(
    app: &App,
    headers: &HeaderMap,
    query: &str,
) -> Result<(String, MemoryQuery), Response> {
    let owner_id = memory_owner(app, headers, query).await?;
    let parsed = parse_query(query).map_err(|text| message(StatusCode::BAD_REQUEST, text))?;
    Ok((owner_id, parsed))
}

async fn memory_owner(app: &App, headers: &HeaderMap, query: &str) -> Result<String, Response> {
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

fn parse_query(query: &str) -> Result<MemoryQuery, &'static str> {
    let pairs = query_pairs(query);
    let saved = match pair(&pairs, "isSaved").as_deref() {
        None => None,
        Some("true") => Some(1),
        Some("false") => Some(0),
        Some(_) => return Err("Validation failed"),
    };
    let trashed = match pair(&pairs, "isTrashed").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err("Validation failed"),
    };
    let upcoming = match pair(&pairs, "isUpcoming").as_deref() {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(_) => return Err("Validation failed"),
    };
    let kind = match pair(&pairs, "type").as_deref() {
        None => None,
        Some(kind @ ("on_this_day" | "birthday")) => Some(kind.to_string()),
        Some(_) => return Err("Validation failed"),
    };
    let id = match pair(&pairs, "id") {
        None => None,
        Some(id) if Uuid::parse_str(&id).is_ok() => Some(id),
        Some(_) => return Err("Validation failed"),
    };
    let for_instant = match pair(&pairs, "for") {
        None => None,
        Some(day) if is_date(&day) => Some(format!("{day}T00:00:00.000Z")),
        Some(_) => return Err("Validation failed"),
    };
    let size = match pair(&pairs, "size") {
        None => None,
        Some(value) => match value.parse::<i64>() {
            Ok(size) if size >= 1 => Some(size),
            _ => return Err("Validation failed"),
        },
    };
    let page = match pair(&pairs, "page") {
        None => None,
        Some(value) => match value.parse::<i64>() {
            Ok(page) if page >= 1 => Some(page),
            _ => return Err("Validation failed"),
        },
    };
    let order = match pair(&pairs, "order").as_deref() {
        None | Some("desc") => MemoryOrder::Desc,
        Some("asc") => MemoryOrder::Asc,
        Some("random") => MemoryOrder::Random,
        Some(_) => return Err("Validation failed"),
    };
    let limit = size.unwrap_or(-1);
    let offset = match (page, size) {
        (Some(page), Some(size)) => (page - 1).saturating_mul(size),
        _ => 0,
    };
    Ok(MemoryQuery {
        trashed,
        saved,
        kind,
        id,
        for_instant,
        upcoming,
        now: crate::db::now_iso(),
        order,
        limit,
        offset,
    })
}

fn is_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes.iter().enumerate().all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
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

fn memory_error(error: MemoryError) -> Response {
    match error {
        MemoryError::Db(error) => db_error(error),
        MemoryError::Status(status, text) => message(status, text),
    }
}
