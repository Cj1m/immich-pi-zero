//! Map, folder, and the search reads beside smart search.

use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::Value;

use crate::browse::{BrowseError, MarkerQuery};
use crate::db::App;
use crate::http::{cookie, db_error, json_ok, message, require_session};
use crate::http_shares::{library_auth, ShareQuery};

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/map/markers", get(map_markers))
        .route("/api/search/cities", get(cities))
        .route("/api/search/explore", get(explore))
        .route("/api/search/places", get(places))
        .route("/api/search/suggestions", get(suggestions))
        .route("/api/search/large-assets", post(large_assets))
        .route("/api/view/folder/unique-paths", get(unique_paths))
        .route("/api/view/folder", get(folder))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MarkerParams {
    is_archived: Option<String>,
    is_favorite: Option<String>,
    file_created_after: Option<String>,
    file_created_before: Option<String>,
    with_partners: Option<String>,
    with_shared_albums: Option<String>,
    key: Option<String>,
    slug: Option<String>,
}

async fn map_markers(
    State(app): State<App>,
    Query(params): Query<MarkerParams>,
    headers: HeaderMap,
) -> Response {
    let share = ShareQuery {
        key: params.key.clone(),
        slug: params.slug.clone(),
        ..ShareQuery::default()
    };
    let user_id = match viewer(&app, &headers, &share).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let query = match marker_query(&params) {
        Ok(query) => query,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.library_markers(&user_id, &query).await {
        Ok(body) => json_ok(body),
        Err(error) => browse_error(error),
    }
}

async fn cities(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let share = share_from_query(request.uri().query().unwrap_or(""));
    let user_id = match viewer(&app, &headers, &share).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.search_cities(&user_id).await {
        Ok(body) => json_ok(body),
        Err(error) => browse_error(error),
    }
}

async fn explore(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let share = share_from_query(request.uri().query().unwrap_or(""));
    let user_id = match viewer(&app, &headers, &share).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.explore(&user_id).await {
        Ok(body) => json_ok(body),
        Err(error) => browse_error(error),
    }
}

async fn places(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let pairs = query_pairs(&raw_query);
    let share = ShareQuery {
        key: pair(&pairs, "key"),
        slug: pair(&pairs, "slug"),
        ..ShareQuery::default()
    };
    let user_id = match viewer(&app, &headers, &share).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let _ = user_id;
    let Some(name) = pair(&pairs, "name") else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    match app.search_places(&name).await {
        Ok(body) => json_ok(body),
        Err(error) => browse_error(error),
    }
}

async fn suggestions(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let pairs = query_pairs(&raw_query);
    let share = ShareQuery {
        key: pair(&pairs, "key"),
        slug: pair(&pairs, "slug"),
        ..ShareQuery::default()
    };
    let user_id = match viewer(&app, &headers, &share).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let Some(kind) = pair(&pairs, "type") else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let include_null = match pair(&pairs, "includeNull").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    match app.suggestions(&user_id, &kind, include_null).await {
        Ok(body) => json_ok(body),
        Err(error) => browse_error(error),
    }
}

async fn large_assets(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let pairs = query_pairs(&raw_query);
    let share = ShareQuery {
        key: pair(&pairs, "key"),
        slug: pair(&pairs, "slug"),
        ..ShareQuery::default()
    };
    let user_id = match viewer(&app, &headers, &share).await {
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
    match app.large_assets(&user_id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => browse_error(error),
    }
}

async fn unique_paths(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let share = share_from_query(request.uri().query().unwrap_or(""));
    let user_id = match viewer(&app, &headers, &share).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.folder_paths(&user_id).await {
        Ok(body) => json_ok(body),
        Err(error) => browse_error(error),
    }
}

async fn folder(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let pairs = query_pairs(&raw_query);
    let share = ShareQuery {
        key: pair(&pairs, "key"),
        slug: pair(&pairs, "slug"),
        ..ShareQuery::default()
    };
    let user_id = match viewer(&app, &headers, &share).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let path = pair(&pairs, "path").unwrap_or_default();
    match app.folder_assets(&user_id, &path).await {
        Ok(body) => json_ok(body),
        Err(error) => browse_error(error),
    }
}

fn share_from_query(query: &str) -> ShareQuery {
    let pairs = query_pairs(query);
    ShareQuery {
        key: pair(&pairs, "key"),
        slug: pair(&pairs, "slug"),
        ..ShareQuery::default()
    }
}

async fn viewer(app: &App, headers: &HeaderMap, query: &ShareQuery) -> Result<String, Response> {
    let share_header = headers.get("x-immich-share-key").is_some() || headers.get("x-immich-share-slug").is_some();
    if query.key.is_some() || query.slug.is_some() || share_header {
        return match library_auth(app, headers, query, false).await {
            Err(response) => Err(response),
            Ok(_) => Err(message(StatusCode::FORBIDDEN, "Forbidden")),
        };
    }
    if cookie(headers, "immich_access_token").is_none() && cookie(headers, "immich_shared_link_token").is_some() {
        return Err(message(StatusCode::FORBIDDEN, "Forbidden"));
    }
    Ok(require_session(app, headers).await?.user.id)
}

fn marker_query(params: &MarkerParams) -> Result<MarkerQuery, &'static str> {
    Ok(MarkerQuery {
        archived: flag(params.is_archived.as_deref())?,
        favorite: match params.is_favorite.as_deref() {
            None => None,
            Some("true") => Some(1),
            Some("false") => Some(0),
            Some(_) => return Err("Validation failed"),
        },
        created_after: params.file_created_after.clone(),
        created_before: params.file_created_before.clone(),
        shared_albums: flag(params.with_shared_albums.as_deref())?,
    })
    .and_then(|query| {
        flag(params.with_partners.as_deref())?;
        Ok(query)
    })
}

fn flag(value: Option<&str>) -> Result<bool, &'static str> {
    match value {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
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

fn browse_error(error: BrowseError) -> Response {
    match error {
        BrowseError::Db(error) => db_error(error),
        BrowseError::Status(status, text) => message(status, text),
    }
}
