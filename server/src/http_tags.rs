//! Tag HTTP. A share key or a shared-link cookie is 403. The access-token cookie is the session.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::Value;

use crate::db::App;
use crate::http::{db_error, json_ok, message};
use crate::http_duplicates::signed_in;

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route(
            "/api/tags",
            get(list_tags).put(upsert_tags).post(create_tag),
        )
        .route("/api/tags/assets", axum::routing::put(bulk_tag_assets))
        .route(
            "/api/tags/{id}",
            get(get_tag)
                .put(update_tag)
                .patch(update_tag)
                .delete(delete_tag),
        )
        .route(
            "/api/tags/{id}/assets",
            axum::routing::put(add_tag_assets).delete(remove_tag_assets),
        )
}

async fn list_tags(State(app): State<App>, request: Request) -> Response {
    let (headers, raw_query) = session_parts(&request);
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.list_tags(&user_id).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn get_tag(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let (headers, raw_query) = session_parts(&request);
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.get_tag(&user_id, &id).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn create_tag(State(app): State<App>, request: Request) -> Response {
    let (user_id, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.create_tag(&user_id, &body).await {
        Ok(body) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Err(error) => db_error(error),
    }
}

async fn update_tag(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let (user_id, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.update_tag(&user_id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn upsert_tags(State(app): State<App>, request: Request) -> Response {
    let (user_id, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.upsert_tags(&user_id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn delete_tag(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let (headers, raw_query) = session_parts(&request);
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.delete_tag(&user_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error),
    }
}

async fn bulk_tag_assets(State(app): State<App>, request: Request) -> Response {
    let (user_id, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.bulk_tag_assets(&user_id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn add_tag_assets(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (user_id, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.add_tag_assets(&user_id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn remove_tag_assets(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (user_id, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.remove_tag_assets(&user_id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

fn session_parts(request: &Request) -> (HeaderMap, String) {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    (headers, raw_query)
}

async fn json_body(app: &App, request: Request) -> Result<(String, Value), Response> {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let user_id = signed_in(app, &headers, &raw_query).await?;
    let bytes = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .map_err(|_| message(StatusCode::BAD_REQUEST, "Validation failed"))?;
    let body = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) if value.is_object() => value,
        _ => return Err(message(StatusCode::BAD_REQUEST, "Validation failed")),
    };
    Ok((user_id, body))
}

fn is_uuid_v4(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.get_version() == Some(uuid::Version::Random))
}
