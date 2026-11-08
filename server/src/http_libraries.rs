//! External library routes. A shared link is 403. A scan does not import files.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::Value;

use crate::db::{App, DbError};
use crate::http::{db_error, message};
use crate::libraries::LibraryFail;

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/libraries", get(list_libraries).post(create_library))
        .route("/api/libraries/{id}/statistics", get(library_statistics))
        .route("/api/libraries/{id}/validate", post(validate_library))
        .route("/api/libraries/{id}/scan", post(scan_library))
        .route(
            "/api/libraries/{id}",
            get(library_one)
                .put(update_library)
                .patch(update_library)
                .delete(delete_library),
        )
}

fn library_error(error: LibraryFail) -> Response {
    match error {
        LibraryFail::Db(error) => db_error(error),
        LibraryFail::BadRequest(text) => message(StatusCode::BAD_REQUEST, text),
    }
}

async fn library_admin(app: &App, headers: &HeaderMap, query: &str) -> Result<(), Response> {
    let user_id = crate::http_duplicates::signed_in(app, headers, query).await?;
    let admin: Option<i64> = sqlx::query_scalar("SELECT isAdmin FROM user WHERE id = ?")
        .bind(user_id)
        .fetch_optional(app.pool())
        .await
        .map_err(|error| db_error(DbError::Database(error)))?;
    if admin == Some(1) {
        Ok(())
    } else {
        Err(message(StatusCode::FORBIDDEN, "Forbidden"))
    }
}

async fn read_json(request: Request) -> Result<Value, Response> {
    let bytes = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .map_err(|_| message(StatusCode::BAD_REQUEST, "Validation failed"))?;
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| message(StatusCode::BAD_REQUEST, "Validation failed"))
}

fn is_uuid_v4(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.get_version() == Some(uuid::Version::Random))
}

async fn list_libraries(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    if let Err(response) = library_admin(&app, &headers, &query).await {
        return response;
    }
    match app.library_list().await {
        Ok(body) => (StatusCode::OK, axum::Json(body)).into_response(),
        Err(error) => library_error(error),
    }
}

async fn create_library(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    if let Err(response) = library_admin(&app, &headers, &query).await {
        return response;
    }
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    match app.create_library(&body).await {
        Ok(body) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Err(error) => library_error(error),
    }
}

async fn library_one(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    if let Err(response) = library_admin(&app, &headers, &query).await {
        return response;
    }
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.library_one(&id).await {
        Ok(body) => (StatusCode::OK, axum::Json(body)).into_response(),
        Err(error) => library_error(error),
    }
}

async fn update_library(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    if let Err(response) = library_admin(&app, &headers, &query).await {
        return response;
    }
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    match app.update_library(&id, &body).await {
        Ok(body) => (StatusCode::OK, axum::Json(body)).into_response(),
        Err(error) => library_error(error),
    }
}

async fn delete_library(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    if let Err(response) = library_admin(&app, &headers, &query).await {
        return response;
    }
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.delete_library(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => library_error(error),
    }
}

async fn validate_library(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    if let Err(response) = library_admin(&app, &headers, &query).await {
        return response;
    }
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    match app.validate_library(&body).await {
        Ok(body) => (StatusCode::OK, axum::Json(body)).into_response(),
        Err(error) => library_error(error),
    }
}

async fn library_statistics(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    if let Err(response) = library_admin(&app, &headers, &query).await {
        return response;
    }
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.library_statistics(&id).await {
        Ok(body) => (StatusCode::OK, axum::Json(body)).into_response(),
        Err(error) => library_error(error),
    }
}

async fn scan_library(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    if let Err(response) = library_admin(&app, &headers, &query).await {
        return response;
    }
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.scan_library(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => library_error(error),
    }
}
