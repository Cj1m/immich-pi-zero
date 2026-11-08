//! HTTP for shared links, the recipient timeline, and downloads.

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::db::App;
use crate::http::{cookie, db_error, json_ok, message, require_session};
use crate::shares::{ApiError, ShareGrant, ShareLookup, Viewer};

#[derive(Deserialize, Default)]
pub(crate) struct ShareQuery {
    pub key: Option<String>,
    pub slug: Option<String>,
    #[serde(rename = "albumId")]
    pub album_id: Option<String>,
    pub id: Option<String>,
    pub order: Option<String>,
    #[serde(rename = "timeBucket")]
    pub time_bucket: Option<String>,
    #[serde(rename = "withCoordinates")]
    pub with_coordinates: Option<String>,
    pub size: Option<String>,
}

pub(crate) enum LibraryAuth {
    Share(Viewer),
    User(String),
}

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/shared-links/login", post(login_link))
        .route("/api/shared-links/me", get(current_link))
        .route("/api/shared-links", get(list_links).post(create_link))
        .route(
            "/api/shared-links/{id}",
            get(get_link).patch(update_link).delete(delete_link),
        )
        .route(
            "/api/shared-links/{id}/assets",
            put(add_link_assets).delete(remove_link_assets),
        )
        .route("/api/timeline/buckets", get(timeline_buckets))
        .route("/api/timeline/bucket", get(timeline_bucket))
        .route("/api/albums/{id}/map-markers", get(map_markers))
        .route("/api/download/info", post(download_info))
        .route("/api/download/archive", post(download_archive))
        .route("/api/assets/{id}/video/playback", get(play_video))
}

pub(crate) fn from_api(error: ApiError) -> Response {
    match error {
        ApiError::Db(error) => db_error(error),
        ApiError::Message(status, text) => message(status, text),
    }
}

pub(crate) fn share_params(
    headers: &HeaderMap,
    query: &ShareQuery,
) -> (Option<String>, Option<String>) {
    let key = header_or_query(headers, "x-immich-share-key", query.key.as_deref());
    let slug = header_or_query(headers, "x-immich-share-slug", query.slug.as_deref());
    (key, slug)
}

/// A `key` or `slug` is the shared link, even when the access-token cookie is also present.
pub(crate) async fn library_auth(
    app: &App,
    headers: &HeaderMap,
    query: &ShareQuery,
    allows_share: bool,
) -> Result<LibraryAuth, Response> {
    let (key, slug) = share_params(headers, query);
    if let Some(key) = key {
        return share_auth(app, true, &key, allows_share).await;
    }
    if let Some(slug) = slug {
        return share_auth(app, false, &slug, allows_share).await;
    }
    let session = require_session(app, headers).await?;
    Ok(LibraryAuth::User(session.user.id))
}

pub(crate) fn user_id(auth: &LibraryAuth) -> &str {
    match auth {
        LibraryAuth::Share(viewer) => &viewer.user_id,
        LibraryAuth::User(id) => id,
    }
}

pub(crate) fn grant(auth: &LibraryAuth) -> Option<&ShareGrant> {
    match auth {
        LibraryAuth::Share(viewer) => Some(&viewer.share),
        LibraryAuth::User(_) => None,
    }
}

async fn share_auth(
    app: &App,
    by_key: bool,
    token: &str,
    allows_share: bool,
) -> Result<LibraryAuth, Response> {
    let lookup = app.lookup_share(by_key, token).await.map_err(db_error)?;
    let ShareLookup::Found(viewer) = lookup else {
        let text = if by_key {
            "Invalid share key"
        } else {
            "Invalid share slug"
        };
        return Err(message(StatusCode::UNAUTHORIZED, text));
    };
    if !allows_share {
        return Err(message(StatusCode::FORBIDDEN, "Forbidden"));
    }
    Ok(LibraryAuth::Share(viewer))
}

async fn list_links(
    State(app): State<App>,
    Query(query): Query<ShareQuery>,
    headers: HeaderMap,
) -> Response {
    let LibraryAuth::User(user_id) = (match library_auth(&app, &headers, &query, false).await {
        Ok(auth) => auth,
        Err(response) => return response,
    }) else {
        return message(StatusCode::FORBIDDEN, "Forbidden");
    };
    match app
        .list_links(&user_id, query.album_id.as_deref(), query.id.as_deref())
        .await
    {
        Ok(body) => json_ok(body),
        Err(error) => from_api(error),
    }
}

async fn create_link(State(app): State<App>, request: Request) -> Response {
    let (user_id, body) = match user_json(&app, request, false).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.create_link(&user_id, &body).await {
        Ok(body) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Err(error) => from_api(error),
    }
}

async fn get_link(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(query): Query<ShareQuery>,
    headers: HeaderMap,
) -> Response {
    let LibraryAuth::User(user_id) = (match library_auth(&app, &headers, &query, false).await {
        Ok(auth) => auth,
        Err(response) => return response,
    }) else {
        return message(StatusCode::FORBIDDEN, "Forbidden");
    };
    match app.link_by_id(&user_id, &id).await {
        Ok(body) => json_ok(body),
        Err(error) => from_api(error),
    }
}

async fn update_link(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let (user_id, body) = match user_json(&app, request, false).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.update_link(&user_id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => from_api(error),
    }
}

async fn delete_link(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(query): Query<ShareQuery>,
    headers: HeaderMap,
) -> Response {
    let LibraryAuth::User(user_id) = (match library_auth(&app, &headers, &query, false).await {
        Ok(auth) => auth,
        Err(response) => return response,
    }) else {
        return message(StatusCode::FORBIDDEN, "Forbidden");
    };
    match app.delete_link(&user_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => from_api(error),
    }
}

async fn current_link(
    State(app): State<App>,
    Query(query): Query<ShareQuery>,
    headers: HeaderMap,
) -> Response {
    let viewer = match require_link(&app, &headers, &query).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    if let Some(password) = viewer.share.password.as_deref() {
        let token = App::password_token(&viewer.share.id, password);
        let tokens = cookie(&headers, "immich_shared_link_token").unwrap_or_default();
        if !tokens.split(',').any(|item| item == token) {
            return message(StatusCode::UNAUTHORIZED, "Password required");
        }
    }
    let strip = !viewer.share.show_metadata;
    match app.public_link(&viewer, strip).await {
        Ok(body) => json_ok(body),
        Err(error) => from_api(error),
    }
}

async fn login_link(State(app): State<App>, request: Request) -> Response {
    let secure = request_is_secure(&request);
    let headers = request.headers().clone();
    let query = query_from_uri(request.uri().query());
    let viewer = match require_link(&app, &headers, &query).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 64 * 1024).await else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let Ok(body) = serde_json::from_slice::<Value>(&bytes) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let Some(password) = body.get("password").and_then(Value::as_str) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let Some(stored) = viewer.share.password.as_deref() else {
        return message(
            StatusCode::BAD_REQUEST,
            "Shared link is not password protected",
        );
    };
    if stored != password {
        return message(StatusCode::UNAUTHORIZED, "Invalid password");
    }
    let strip = !viewer.share.show_metadata;
    let body = match app.public_link(&viewer, strip).await {
        Ok(body) => body,
        Err(error) => return from_api(error),
    };
    let token = App::password_token(&viewer.share.id, stored);
    let mut tokens: Vec<String> = cookie(&headers, "immich_shared_link_token")
        .unwrap_or_default()
        .split(',')
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    if !tokens.iter().any(|item| item == &token) {
        tokens.push(token);
    }
    let mut response = (StatusCode::CREATED, axum::Json(body)).into_response();
    let secure_flag = if secure { "; Secure" } else { "" };
    let cookie_value = format!(
        "immich_shared_link_token={}; HttpOnly; Path=/; SameSite=Lax; Max-Age=86400{secure_flag}",
        tokens.join(",")
    );
    if let Ok(value) = HeaderValue::from_str(&cookie_value) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

async fn add_link_assets(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (user_id, body) = match user_json(&app, request, false).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let ids = string_ids(body.get("assetIds"));
    match app.add_link_assets(&user_id, &id, &ids).await {
        Ok(body) => json_ok(body),
        Err(error) => from_api(error),
    }
}

async fn remove_link_assets(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (user_id, body) = match user_json(&app, request, false).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let ids = string_ids(body.get("assetIds"));
    match app.remove_link_assets(&user_id, &id, &ids).await {
        Ok(body) => json_ok(body),
        Err(error) => from_api(error),
    }
}

async fn timeline_buckets(
    State(app): State<App>,
    Query(query): Query<ShareQuery>,
    headers: HeaderMap,
) -> Response {
    let auth = match library_auth(&app, &headers, &query, true).await {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    match app
        .timeline_buckets(
            user_id(&auth),
            grant(&auth),
            query.album_id.as_deref(),
            query.order.as_deref().unwrap_or("desc"),
        )
        .await
    {
        Ok(body) => json_ok(body),
        Err(error) => from_api(error),
    }
}

async fn timeline_bucket(
    State(app): State<App>,
    Query(query): Query<ShareQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(time_bucket) = query.time_bucket.clone() else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let auth = match library_auth(&app, &headers, &query, true).await {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    let coordinates = matches!(query.with_coordinates.as_deref(), Some("true" | "1"));
    match app
        .timeline_bucket(
            user_id(&auth),
            grant(&auth),
            query.album_id.as_deref(),
            query.order.as_deref().unwrap_or("desc"),
            &time_bucket,
            coordinates,
        )
        .await
    {
        Ok(body) => json_ok(body),
        Err(error) => from_api(error),
    }
}

async fn map_markers(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(query): Query<ShareQuery>,
    headers: HeaderMap,
) -> Response {
    let auth = match library_auth(&app, &headers, &query, true).await {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    match app.map_markers(user_id(&auth), grant(&auth), &id).await {
        Ok(body) => json_ok(body),
        Err(error) => from_api(error),
    }
}

async fn download_info(State(app): State<App>, request: Request) -> Response {
    let (auth, body) = match auth_json(&app, request, true).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.download_info(user_id(&auth), grant(&auth), &body).await {
        Ok(body) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Err(error) => from_api(error),
    }
}

async fn download_archive(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = query_from_uri(request.uri().query());
    let auth = match library_auth(&app, &headers, &query, true).await {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024 * 1024).await else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let form = String::from_utf8_lossy(&bytes);
    let asset_ids = form_value(&form, "assetIds").unwrap_or_default();
    let ids: Vec<String> = asset_ids
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    let archive_name = form_value(&form, "archiveName").unwrap_or_else(|| "immich".to_string());
    let zip = match app.download_zip(user_id(&auth), grant(&auth), &ids).await {
        Ok(bytes) => bytes,
        Err(error) => return from_api(error),
    };
    let disposition = zip_disposition(&archive_name);
    let length = zip.len();
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/zip")],
        zip,
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).expect("length"),
    );
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

async fn play_video(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(query): Query<ShareQuery>,
    headers: HeaderMap,
) -> Response {
    let auth = match library_auth(&app, &headers, &query, true).await {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    if let Some(grant) = grant(&auth) {
        match app.share_can_see(user_id(&auth), grant, &id, false).await {
            Ok(true) => {}
            Ok(false) => {
                return message(StatusCode::BAD_REQUEST, "Not found or no asset.view access")
            }
            Err(error) => return db_error(error),
        }
    }
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    match app.original_response(user_id(&auth), &id, range).await {
        Ok(response) => response,
        Err(error) => db_error(error),
    }
}

async fn require_link(
    app: &App,
    headers: &HeaderMap,
    query: &ShareQuery,
) -> Result<Viewer, Response> {
    match library_auth(app, headers, query, true).await? {
        LibraryAuth::Share(viewer) => Ok(viewer),
        LibraryAuth::User(_) => Err(message(StatusCode::FORBIDDEN, "Forbidden")),
    }
}

async fn user_json(
    app: &App,
    request: Request,
    allows_share: bool,
) -> Result<(String, Value), Response> {
    let (auth, body) = auth_json(app, request, allows_share).await?;
    match auth {
        LibraryAuth::User(id) => Ok((id, body)),
        LibraryAuth::Share(_) => Err(message(StatusCode::FORBIDDEN, "Forbidden")),
    }
}

async fn auth_json(
    app: &App,
    request: Request,
    allows_share: bool,
) -> Result<(LibraryAuth, Value), Response> {
    let headers = request.headers().clone();
    let query = query_from_uri(request.uri().query());
    let auth = library_auth(app, &headers, &query, allows_share).await?;
    let bytes = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .map_err(|_| message(StatusCode::BAD_REQUEST, "Validation failed"))?;
    let body = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes)
            .map_err(|_| message(StatusCode::BAD_REQUEST, "Validation failed"))?
    };
    Ok((auth, body))
}

fn header_or_query(headers: &HeaderMap, name: &str, query: Option<&str>) -> Option<String> {
    let header = headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    header.or_else(|| query.filter(|value| !value.is_empty()).map(str::to_string))
}

fn query_from_uri(query: Option<&str>) -> ShareQuery {
    let Some(query) = query else {
        return ShareQuery::default();
    };
    let mut parsed = ShareQuery::default();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = decode_form(value);
        match decode_form(key).as_str() {
            "key" => parsed.key = Some(value),
            "slug" => parsed.slug = Some(value),
            "albumId" => parsed.album_id = Some(value),
            "id" => parsed.id = Some(value),
            "order" => parsed.order = Some(value),
            "timeBucket" => parsed.time_bucket = Some(value),
            "withCoordinates" => parsed.with_coordinates = Some(value),
            "size" => parsed.size = Some(value),
            _ => {}
        }
    }
    parsed
}

fn string_ids(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn form_value(body: &str, name: &str) -> Option<String> {
    for pair in body.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if decode_form(key) == name {
            return Some(decode_form(value));
        }
    }
    None
}

fn decode_form(value: &str) -> String {
    let value = value.replace('+', " ");
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            ) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn zip_disposition(name: &str) -> String {
    let mut name = name.replace(['/', '\\', '"'], "_");
    if !name.to_ascii_lowercase().ends_with(".zip") {
        name.push_str(".zip");
    }
    let encoded = percent_encode(&name);
    format!("attachment; filename*=UTF-8''{encoded}")
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn request_is_secure(request: &Request) -> bool {
    if request.uri().scheme_str() == Some("https") {
        return true;
    }
    request
        .headers()
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("https"))
}
