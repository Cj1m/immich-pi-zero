//! People list, person search, hide and favorite, and people-user shares.

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde_json::Value;

use crate::db::App;
use crate::http::{cookie, db_error, json_ok, message, require_session};
use crate::http_shares::{library_auth, ShareQuery};
use crate::people::{PeopleError, PeopleQuery, PeopleUserQuery};

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/people", get(list_people).post(create_person))
        .route("/api/people/merge", post(merge_people))
        .route(
            "/api/people/users",
            get(list_users).put(upsert_users).delete(remove_users),
        )
        .route("/api/people/{id}/reassign", put(reassign_faces))
        .route("/api/people/{id}/statistics", get(statistics))
        .route("/api/people/{id}/thumbnail", get(thumbnail))
        .route(
            "/api/people/{id}",
            get(person).put(update_person).patch(update_person),
        )
        .route("/api/faces", get(asset_faces).post(create_face))
        .route("/api/faces/{id}", put(reassign_face).delete(delete_face))
        .route("/api/search/person", get(search_person))
}

async fn asset_faces(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let caller = match crate::http_duplicates::signed_in(&app, &headers, &raw_query).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Some(id) = query_pairs(&raw_query)
        .into_iter()
        .find(|(name, _)| name == "id")
        .map(|(_, value)| value)
        .filter(|id| !id.is_empty())
    else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    if let Err(error) = crate::http_media::readable_asset(&app, &caller, &id, session.elevated).await {
        return crate::http_media::asset_read_response(error);
    }
    match app.asset_faces(&id, &caller).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn create_face(State(app): State<App>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let query = parts.uri.query().unwrap_or("");
    let caller = match crate::http_duplicates::signed_in(&app, &parts.headers, query).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let bytes = match axum::body::to_bytes(body, 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    let value = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) if value.is_object() => value,
        _ => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    match app.create_manual_face(&caller, &value).await {
        Ok(()) => StatusCode::CREATED.into_response(),
        Err(error) => people_error(error),
    }
}

async fn list_people(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let query = match people_query(&raw_query) {
        Ok(query) => query,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.list_people(&user_id, &query).await {
        Ok(body) => json_ok(body),
        Err(error) => people_error(error),
    }
}

async fn list_users(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let query = match people_user_query(&raw_query) {
        Ok(query) => query,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.list_person_users(&user_id, &query).await {
        Ok(body) => json_ok(body),
        Err(error) => people_error(error),
    }
}

async fn upsert_users(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.upsert_people_users(&user_id, &body).await {
        Ok(()) => empty_ok(),
        Err(error) => people_error(error),
    }
}

async fn remove_users(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.delete_people_users(&user_id, &body).await {
        Ok(()) => empty_ok(),
        Err(error) => people_error(error),
    }
}

async fn statistics(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.person_statistics(&user_id, &id).await {
        Ok(body) => json_ok(body),
        Err(error) => people_error(error),
    }
}

async fn thumbnail(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.person_thumbnail(&user_id, &id).await {
        Ok((content_type, bytes)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type),
                (
                    header::CACHE_CONTROL,
                    "private, no-cache, no-transform".to_string(),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(error) => people_error(error),
    }
}

async fn person(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.person_document(&user_id, &id).await {
        Ok(body) => json_ok(body),
        Err(error) => people_error(error),
    }
}

async fn update_person(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.update_person(&user_id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => people_error(error),
    }
}

async fn create_person(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.create_person(&user_id, &body).await {
        Ok(body) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Err(error) => people_error(error),
    }
}

async fn merge_people(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.merge_people(&user_id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => people_error(error),
    }
}

async fn reassign_faces(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.reassign_faces(&user_id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => people_error(error),
    }
}

async fn reassign_face(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.reassign_face(&user_id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => people_error(error),
    }
}

async fn delete_face(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    if uuid::Uuid::parse_str(&id).is_err() {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(text) => return message(StatusCode::BAD_REQUEST, text),
    };
    match app.delete_face(&user_id, &id, &body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => people_error(error),
    }
}

async fn search_person(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let pairs = query_pairs(&raw_query);
    let user_id = match viewer(&app, &headers, &share_from_query(&raw_query)).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let Some(name) = pair(&pairs, "name") else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let with_hidden = match pair(&pairs, "withHidden").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    match app.search_people(&user_id, &name, with_hidden).await {
        Ok(body) => json_ok(body),
        Err(error) => people_error(error),
    }
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

fn people_query(query: &str) -> Result<PeopleQuery, &'static str> {
    let pairs = query_pairs(query);
    if let Some(id) = pair(&pairs, "closestPersonId") {
        uuid_text(&id)?;
    }
    if let Some(id) = pair(&pairs, "closestAssetId") {
        uuid_text(&id)?;
    }
    Ok(PeopleQuery {
        with_hidden: flag(pair(&pairs, "withHidden").as_deref())?,
        page: optional_int(pair(&pairs, "page").as_deref(), 1, 1, i64::MAX)?,
        size: optional_int(pair(&pairs, "size").as_deref(), 500, 1, 1000)?,
        favorite: optional_flag(pair(&pairs, "isFavorite").as_deref())?,
        hidden: optional_flag(pair(&pairs, "isHidden").as_deref())?,
        name: pair(&pairs, "name"),
        shared_by: optional_uuid(pair(&pairs, "sharedById"))?,
        shared_with: optional_uuid(pair(&pairs, "sharedWithId"))?,
    })
}

fn people_user_query(query: &str) -> Result<PeopleUserQuery, &'static str> {
    let pairs = query_pairs(query);
    let shared_by_caller = match pair(&pairs, "direction").as_deref() {
        None => None,
        Some("shared-by") => Some(true),
        Some("shared-with") => Some(false),
        Some(_) => return Err("Validation failed"),
    };
    let role = match pair(&pairs, "role").as_deref() {
        None => None,
        Some(value @ ("read" | "write" | "admin")) => Some(value.to_string()),
        Some(_) => return Err("Validation failed"),
    };
    Ok(PeopleUserQuery {
        person_id: optional_uuid(pair(&pairs, "personId"))?,
        shared_by: optional_uuid(pair(&pairs, "sharedById"))?,
        shared_with: optional_uuid(pair(&pairs, "sharedWithId"))?,
        role,
        shared_by_caller,
    })
}

fn optional_uuid(value: Option<String>) -> Result<Option<String>, &'static str> {
    match value {
        None => Ok(None),
        Some(value) => {
            uuid_text(&value)?;
            Ok(Some(value))
        }
    }
}

fn uuid_text(value: &str) -> Result<(), &'static str> {
    if uuid::Uuid::parse_str(value).is_ok() {
        Ok(())
    } else {
        Err("Validation failed")
    }
}

fn optional_int(
    value: Option<&str>,
    default: i64,
    min: i64,
    max: i64,
) -> Result<i64, &'static str> {
    let Some(value) = value else {
        return Ok(default);
    };
    let parsed = value.parse::<i64>().map_err(|_| "Validation failed")?;
    if (min..=max).contains(&parsed) {
        Ok(parsed)
    } else {
        Err("Validation failed")
    }
}

fn flag(value: Option<&str>) -> Result<bool, &'static str> {
    match value {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err("Validation failed"),
    }
}

fn optional_flag(value: Option<&str>) -> Result<Option<i64>, &'static str> {
    match value {
        None => Ok(None),
        Some("true") => Ok(Some(1)),
        Some("false") => Ok(Some(0)),
        Some(_) => Err("Validation failed"),
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
    let share_header =
        headers.get("x-immich-share-key").is_some() || headers.get("x-immich-share-slug").is_some();
    if query.key.is_some() || query.slug.is_some() || share_header {
        return match library_auth(app, headers, query, false).await {
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

fn people_error(error: PeopleError) -> Response {
    match error {
        PeopleError::Db(error) => db_error(error),
        PeopleError::Status(status, text) => message(status, text),
    }
}

fn empty_ok() -> Response {
    StatusCode::OK.into_response()
}
