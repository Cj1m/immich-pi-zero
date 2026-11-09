//! HTTP for the library: upload, sync, albums, trash, and stacks.

use std::collections::HashMap;
use std::path::PathBuf;

use axum::extract::{Multipart, Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use tokio::io::AsyncWriteExt;

use crate::db::{App, DbError};
use crate::http::{db_error, json_ok, message, require_session};
use crate::http_duplicates::signed_in;
use crate::http_shares::{elevated, grant, library_auth, user_id, LibraryAuth, ShareQuery};
use crate::media::{IncomingOriginal, IngestResult};

pub fn routes() -> Router<App> {
    Router::new()
        .route(
            "/api/assets",
            post(upload_asset)
                .put(bulk_update_assets)
                .patch(bulk_update_assets)
                .delete(delete_assets),
        )
        .route("/api/assets/bulk-upload-check", post(bulk_upload_check))
        .route("/api/assets/jobs", post(asset_jobs))
        .route(
            "/api/assets/{id}",
            get(get_asset).put(update_asset).patch(update_asset),
        )
        .route("/api/assets/{id}/original", get(download_original))
        .route("/api/assets/{id}/thumbnail", get(download_thumbnail))
        .route("/api/assets/{id}/ocr", get(asset_ocr))
        .route(
            "/api/assets/{id}/edits",
            get(asset_edits).put(replace_edits).delete(clear_edits),
        )
        .route("/api/sync/stream", post(sync_stream))
        .route(
            "/api/sync/ack",
            get(sync_acks).post(sync_ack).delete(sync_ack_delete),
        )
        .route("/api/albums", get(list_albums).post(create_album))
        .route(
            "/api/albums/{id}",
            get(get_album).patch(patch_album).delete(delete_album),
        )
        .route(
            "/api/albums/{id}/assets",
            put(add_album_assets).delete(remove_album_assets),
        )
        .route("/api/albums/{id}/users", put(add_album_users))
        .route(
            "/api/albums/{id}/user/{user_id}",
            put(update_album_user).delete(remove_album_user),
        )
        .route("/api/trash/empty", post(empty_trash))
        .route("/api/trash/restore/assets", post(restore_assets))
        .route("/api/trash/restore", post(restore_trash))
        .route("/api/stacks", post(create_stack))
}

async fn upload_asset(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<ShareQuery>,
    mut multipart: Multipart,
) -> Response {
    let (owner_id, share) = match library_auth(&app, &headers, &query, true).await {
        Ok(LibraryAuth::User { user_id: id, .. }) => (id, None),
        Ok(LibraryAuth::Share(viewer)) => {
            if !viewer.share.allow_upload {
                return message(StatusCode::UNAUTHORIZED, "Unauthorized");
            }
            (viewer.user_id, Some(viewer.share))
        }
        Err(response) => return response,
    };
    let quota = match app.quota_size(&owner_id).await {
        Ok(quota) => quota,
        Err(error) => return db_error(error),
    };
    let tmp = app.data_dir().join("tmp");
    if let Err(error) = tokio::fs::create_dir_all(&tmp).await {
        return db_error(DbError::Store(error.to_string()));
    }
    let mut texts = HashMap::new();
    let mut asset: Option<UploadedPart> = None;
    let mut sidecar: Option<PathBuf> = None;
    let mut temps = Vec::new();
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => {
                remove_temps(&temps);
                return message(StatusCode::BAD_REQUEST, "Validation failed");
            }
        };
        let name = field.name().unwrap_or("").to_string();
        let filename = field.file_name().unwrap_or("asset").to_string();
        if name == "assetData" || name == "sidecarData" {
            let uploaded = match write_part(field, &tmp).await {
                Ok(uploaded) => uploaded,
                Err(response) => {
                    remove_temps(&temps);
                    return response;
                }
            };
            temps.push(uploaded.path.clone());
            if name == "assetData" {
                let mut uploaded = uploaded;
                uploaded.filename = filename;
                asset = Some(uploaded);
            } else {
                sidecar = Some(uploaded.path);
            }
        } else {
            let text = match field.text().await {
                Ok(text) => text,
                Err(_) => {
                    remove_temps(&temps);
                    return message(StatusCode::BAD_REQUEST, "Validation failed");
                }
            };
            texts.insert(name, text);
        }
    }
    let Some(asset) = asset else {
        remove_temps(&temps);
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let filename = texts
        .get("filename")
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or(asset.filename);
    let Some(file_created_at) = texts
        .get("fileCreatedAt")
        .filter(|value| value.contains('T'))
    else {
        remove_temps(&temps);
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let Some(file_modified_at) = texts
        .get("fileModifiedAt")
        .filter(|value| value.contains('T'))
    else {
        remove_temps(&temps);
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let visibility = texts
        .get("visibility")
        .map(String::as_str)
        .unwrap_or("timeline");
    if !matches!(visibility, "timeline" | "archive" | "hidden" | "locked") {
        remove_temps(&temps);
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let duration_ms = match texts.get("duration").map(String::as_str) {
        None | Some("") | Some("0") => None,
        Some(value) => match value.parse::<i64>() {
            Ok(number) if number > 0 => Some(number),
            _ => {
                remove_temps(&temps);
                return message(StatusCode::BAD_REQUEST, "Validation failed");
            }
        },
    };
    let is_favorite = matches!(
        texts
            .get("isFavorite")
            .map(String::as_str)
            .unwrap_or("false"),
        "true" | "1"
    );
    let incoming = IncomingOriginal {
        temp_path: asset.path,
        checksum: asset.checksum,
        byte_size: asset.byte_size,
        filename,
        file_created_at: file_created_at.clone(),
        file_modified_at: file_modified_at.clone(),
        is_favorite,
        duration_ms,
        visibility: visibility.to_string(),
        live_photo_video_id: texts
            .get("livePhotoVideoId")
            .filter(|value| !value.is_empty())
            .cloned(),
        sidecar_temp: sidecar,
    };
    let result = app.ingest_original(&owner_id, quota, &incoming).await;
    remove_temps(&temps);
    let asset_id = match &result {
        Ok(IngestResult::Created(id) | IngestResult::Duplicate(id)) => Some(id.clone()),
        Err(_) => None,
    };
    if let (Some(grant), Some(asset_id)) = (share.as_ref(), asset_id) {
        if let Err(error) = app.attach_shared_upload(grant, &asset_id).await {
            return db_error(error);
        }
    }
    match result {
        Ok(IngestResult::Created(id)) => (
            StatusCode::CREATED,
            axum::Json(json!({ "status": "created", "id": id })),
        )
            .into_response(),
        Ok(IngestResult::Duplicate(id)) => json_ok(json!({ "status": "duplicate", "id": id })),
        Err(error) => db_error(error),
    }
}

struct UploadedPart {
    path: PathBuf,
    checksum: Vec<u8>,
    byte_size: i64,
    filename: String,
}

async fn write_part(
    mut field: axum::extract::multipart::Field<'_>,
    directory: &std::path::Path,
) -> Result<UploadedPart, Response> {
    let path = directory.join(format!("upload-{}", crate::db::new_id()));
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(|error| db_error(DbError::Store(error.to_string())))?;
    let mut hasher = Sha1::new();
    let mut byte_size = 0_i64;
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|_| message(StatusCode::BAD_REQUEST, "Validation failed"))?
    {
        hasher.update(&chunk);
        byte_size += chunk.len() as i64;
        file.write_all(&chunk)
            .await
            .map_err(|error| db_error(DbError::Store(error.to_string())))?;
    }
    Ok(UploadedPart {
        path,
        checksum: hasher.finalize().to_vec(),
        byte_size,
        filename: "asset".to_string(),
    })
}

fn remove_temps(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

async fn bulk_upload_check(State(app): State<App>, request: Request) -> Response {
    let (user_id, body) = match signed_json(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.bulk_upload_check(&user_id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn asset_jobs(State(app): State<App>, request: Request) -> Response {
    let (user_id, body) = match signed_json(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.run_asset_job(&user_id, &body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn get_asset(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(query): Query<ShareQuery>,
    headers: HeaderMap,
) -> Response {
    let auth = match library_auth(&app, &headers, &query, true).await {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    if let Some(link) = grant(&auth) {
        match app.share_can_see(user_id(&auth), link, &id, false).await {
            Ok(true) => {}
            Ok(false) => {
                return message(StatusCode::BAD_REQUEST, "Not found or no asset.read access")
            }
            Err(error) => return db_error(error),
        }
        return match app
            .share_asset_json(user_id(&auth), &id, !link.show_metadata)
            .await
        {
            Ok(body) => json_ok(body),
            Err(error) => db_error(error),
        };
    }
    let caller = user_id(&auth);
    let owner = match app.asset_gate(caller, &id).await {
        Ok(Ok(owner)) => owner,
        Ok(Err(false)) => return message(StatusCode::BAD_REQUEST, "Asset not found"),
        Ok(Err(true)) => {
            return message(StatusCode::BAD_REQUEST, "Not found or no asset.read access")
        }
        Err(error) => return db_error(error),
    };
    if let Err(response) = reject_locked(&app, &owner, &id, elevated(&auth), "asset.read").await {
        return response;
    }
    match app.asset_json(&owner, &id).await {
        Ok(mut body) => {
            if caller != owner {
                match app.asset_people(&id, caller).await {
                    Ok(people) => body["people"] = people,
                    Err(error) => return db_error(error),
                }
            }
            json_ok(body)
        }
        Err(DbError::NotFound) => message(StatusCode::BAD_REQUEST, "Asset not found"),
        Err(error) => db_error(error),
    }
}

async fn asset_edits(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("");
    let caller = match signed_in(&app, &headers, query).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    match app.read_asset_edits(&caller, &id).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn replace_edits(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (caller, body) = match signed_json(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.replace_asset_edits(&caller, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn clear_edits(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("");
    let caller = match signed_in(&app, &headers, query).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    match app.clear_asset_edits(&caller, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn asset_ocr(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("");
    let caller = match signed_in(&app, &headers, query).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if let Err(error) = readable_asset(&app, &caller, &id, session.elevated).await {
        return asset_read_response(error);
    }
    match app.asset_ocr(&id).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

pub(crate) enum AssetRead {
    Validation,
    Missing,
    Denied,
    Database(DbError),
}

pub(crate) fn asset_read_response(error: AssetRead) -> Response {
    match error {
        AssetRead::Validation => message(StatusCode::BAD_REQUEST, "Validation failed"),
        AssetRead::Missing => message(StatusCode::BAD_REQUEST, "Asset not found"),
        AssetRead::Denied => message(StatusCode::BAD_REQUEST, "Not found or no asset.read access"),
        AssetRead::Database(error) => db_error(error),
    }
}

pub(crate) async fn readable_asset(
    app: &App,
    caller: &str,
    asset_id: &str,
    elevated: bool,
) -> Result<String, AssetRead> {
    if uuid::Uuid::parse_str(asset_id).is_err() {
        return Err(AssetRead::Validation);
    }
    let owner = match app.asset_gate(caller, asset_id).await {
        Ok(Ok(owner)) => owner,
        Ok(Err(false)) => return Err(AssetRead::Missing),
        Ok(Err(true)) => return Err(AssetRead::Denied),
        Err(error) => return Err(AssetRead::Database(error)),
    };
    match app.asset_visibility(&owner, asset_id).await {
        Ok(Some(visibility)) if visibility == "locked" && !elevated => Err(AssetRead::Denied),
        Ok(_) => Ok(owner),
        Err(error) => Err(AssetRead::Database(error)),
    }
}

async fn update_asset(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.update_asset(&session.user.id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn bulk_update_assets(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = match crate::http_duplicates::signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let bytes = match axum::body::to_bytes(request.into_body(), 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    let body: Value = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) if value.is_object() => value,
        _ => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    match app.update_assets(&user_id, &body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn delete_assets(State(app): State<App>, request: Request) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(ids) = string_ids(body.get("ids")) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let force = body
        .get("force")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    match app.delete_assets(&session.user.id, &ids, force).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn download_original(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(query): Query<ShareQuery>,
    headers: HeaderMap,
) -> Response {
    let auth = match library_auth(&app, &headers, &query, true).await {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    if let Some(link) = grant(&auth) {
        match app.share_can_see(user_id(&auth), link, &id, true).await {
            Ok(true) => {}
            Ok(false) => {
                return message(
                    StatusCode::BAD_REQUEST,
                    "Not found or no asset.download access",
                )
            }
            Err(error) => return db_error(error),
        }
    } else if let Err(response) =
        reject_locked(&app, user_id(&auth), &id, elevated(&auth), "asset.download").await
    {
        return response;
    }
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    match app.original_response(user_id(&auth), &id, range).await {
        Ok(response) => response,
        Err(error) => db_error(error),
    }
}

async fn download_thumbnail(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(query): Query<ShareQuery>,
    headers: HeaderMap,
) -> Response {
    let auth = match library_auth(&app, &headers, &query, true).await {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    if query.size.as_deref() == Some("fullsize") {
        let mut location = format!("/api/assets/{id}/thumbnail?size=preview");
        if let Some(key) = query.key.as_deref().filter(|value| !value.is_empty()) {
            location.push_str("&key=");
            location.push_str(key);
        } else if let Some(slug) = query.slug.as_deref().filter(|value| !value.is_empty()) {
            location.push_str("&slug=");
            location.push_str(slug);
        }
        return (StatusCode::FOUND, [(header::LOCATION, location)]).into_response();
    }
    if let Some(link) = grant(&auth) {
        match app.share_can_see(user_id(&auth), link, &id, false).await {
            Ok(true) => {}
            Ok(false) => {
                return message(StatusCode::BAD_REQUEST, "Not found or no asset.view access")
            }
            Err(error) => return db_error(error),
        }
    } else if let Err(response) =
        reject_locked(&app, user_id(&auth), &id, elevated(&auth), "asset.view").await
    {
        return response;
    }
    let size = query
        .size
        .clone()
        .unwrap_or_else(|| "thumbnail".to_string());
    let edited = matches!(query.edited.as_deref(), Some("true") | Some("1"));
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    match app
        .thumbnail_response(user_id(&auth), &id, &size, edited, range)
        .await
    {
        Ok(response) => response,
        Err(error) => db_error(error),
    }
}

async fn reject_locked(
    app: &App,
    owner_id: &str,
    asset_id: &str,
    elevated: bool,
    permission: &str,
) -> Result<(), Response> {
    if elevated {
        return Ok(());
    }
    let visibility = app
        .asset_visibility(owner_id, asset_id)
        .await
        .map_err(db_error)?;
    if visibility.as_deref() == Some("locked") {
        return Err(message(
            StatusCode::BAD_REQUEST,
            format!("Not found or no {permission} access"),
        ));
    }
    Ok(())
}

async fn sync_stream(State(app): State<App>, request: Request) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(types) = body.get("types").and_then(|value| value.as_array()) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let types: Vec<String> = types
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let reset = body
        .get("reset")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if types.iter().any(|kind| kind == "AssetFacesV1") {
        return message(
            StatusCode::BAD_REQUEST,
            "SyncRequestType.AssetFacesV1 is deprecated, use SyncRequestType.AssetFacesV2 instead",
        );
    }
    if types.iter().any(|kind| kind == "PartnerAssetsV1") {
        return message(
            StatusCode::BAD_REQUEST,
            "SyncRequestType.PartnerAssetsV1 is deprecated, use SyncRequestType.PartnerAssetsV2 instead",
        );
    }
    match app
        .sync_stream(&session.session_id, &session.user.id, &types, reset)
        .await
    {
        Ok(body) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/jsonlines+json")],
            body,
        )
            .into_response(),
        Err(error) => db_error(error),
    }
}

async fn sync_acks(State(app): State<App>, headers: HeaderMap) -> Response {
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.sync_ack_list(&session.session_id).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn sync_ack(State(app): State<App>, request: Request) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(acks) = body.get("acks").and_then(|value| value.as_array()) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let acks: Vec<String> = acks
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    if acks.len() != body["acks"].as_array().map(|list| list.len()).unwrap_or(0) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.set_sync_acks(&session.session_id, &acks).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn sync_ack_delete(State(app): State<App>, request: Request) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let types = match body.get("types") {
        None => None,
        Some(value) => match string_ids(Some(value)) {
            Ok(types) => Some(types),
            Err(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
        },
    };
    match app
        .delete_sync_acks(&session.session_id, types.as_deref())
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn list_albums(State(app): State<App>, headers: HeaderMap) -> Response {
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.list_albums(&session.user.id).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn create_album(State(app): State<App>, request: Request) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(name) = body.get("albumName").and_then(|value| value.as_str()) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let description = body.get("description").and_then(|value| value.as_str());
    let asset_ids = match body.get("assetIds") {
        None => Vec::new(),
        Some(value) => match string_ids(Some(value)) {
            Ok(ids) => ids,
            Err(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
        },
    };
    match app
        .create_album(&session.user.id, name, description, &asset_ids)
        .await
    {
        Ok(body) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Err(error) => db_error(error),
    }
}

async fn get_album(State(app): State<App>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.album_json(&session.user.id, &id).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn patch_album(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.patch_album(&session.user.id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn delete_album(
    State(app): State<App>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.delete_album(&session.user.id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn add_album_assets(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(ids) = string_ids(body.get("ids")) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    match app.add_album_assets(&session.user.id, &id, &ids).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn remove_album_assets(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(ids) = string_ids(body.get("ids")) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    match app.remove_album_assets(&session.user.id, &id, &ids).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn add_album_users(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.add_album_users(&session.user.id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn update_album_user(
    State(app): State<App>,
    Path((id, user_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let role = body
        .get("role")
        .and_then(|value| value.as_str())
        .unwrap_or("editor");
    let payload = json!({ "albumUsers": [{ "userId": user_id, "role": role }] });
    match app.add_album_users(&session.user.id, &id, &payload).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn remove_album_user(
    State(app): State<App>,
    Path((id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.remove_album_user(&session.user.id, &id, &user_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn empty_trash(State(app): State<App>, headers: HeaderMap) -> Response {
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.empty_trash(&session.user.id).await {
        Ok(count) => json_ok(json!({ "count": count })),
        Err(error) => db_error(error),
    }
}

async fn restore_trash(State(app): State<App>, headers: HeaderMap) -> Response {
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.restore_assets(&session.user.id, None).await {
        Ok(count) => json_ok(json!({ "count": count })),
        Err(error) => db_error(error),
    }
}

async fn restore_assets(State(app): State<App>, request: Request) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(ids) = string_ids(body.get("ids")) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    match app.restore_assets(&session.user.id, Some(&ids)).await {
        Ok(count) => json_ok(json!({ "count": count })),
        Err(error) => db_error(error),
    }
}

async fn create_stack(State(app): State<App>, request: Request) -> Response {
    let (session, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(ids) = string_ids(body.get("assetIds")) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    match app.create_stack(&session.user.id, &ids).await {
        Ok(body) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Err(error) => db_error(error),
    }
}

/// Session cookie, read after auth. A shared link is 403 before the body is parsed.
async fn signed_json(app: &App, request: Request) -> Result<(String, Value), Response> {
    let (parts, body) = request.into_parts();
    let query = parts.uri.query().unwrap_or("").to_string();
    let user_id = signed_in(app, &parts.headers, &query).await?;
    let bytes = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .map_err(|_| message(StatusCode::BAD_REQUEST, "Validation failed"))?;
    let value = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
    if value.is_null() && !bytes.is_empty() {
        return Err(message(StatusCode::BAD_REQUEST, "Validation failed"));
    }
    Ok((user_id, value))
}

async fn json_body(
    app: &App,
    request: Request,
) -> Result<(crate::db::CurrentSession, Value), Response> {
    let (parts, body) = request.into_parts();
    let session = require_session(app, &parts.headers).await?;
    let bytes = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .map_err(|_| message(StatusCode::BAD_REQUEST, "Validation failed"))?;
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if value.is_null() && !bytes.is_empty() {
        return Err(message(StatusCode::BAD_REQUEST, "Validation failed"));
    }
    Ok((session, value))
}

fn string_ids(value: Option<&Value>) -> Result<Vec<String>, ()> {
    let list = value.and_then(|item| item.as_array()).ok_or(())?;
    let mut ids = Vec::new();
    for item in list {
        ids.push(item.as_str().ok_or(())?.to_string());
    }
    Ok(ids)
}
