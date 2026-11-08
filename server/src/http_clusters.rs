//! Cluster group HTTP. A share key or a shared-link cookie is 403. The access-token cookie is the session.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::Value;

use crate::clusters::ClusterFail;
use crate::db::App;
use crate::http::{db_error, json_ok, message};
use crate::http_duplicates::signed_in;

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/cluster-groups/requests", get(list_my_requests))
        .route(
            "/api/cluster-groups/requests/{id}/accept",
            axum::routing::post(accept_request),
        )
        .route(
            "/api/cluster-groups/requests/{id}",
            axum::routing::delete(delete_request),
        )
        .route(
            "/api/cluster-groups/{id}/requests",
            get(list_group_requests).put(create_request),
        )
        .route("/api/cluster-groups/{id}/users", get(list_group_users))
        .route(
            "/api/cluster-groups/{id}/leave",
            axum::routing::post(leave_group),
        )
        .route(
            "/api/cluster-groups/{id}/regenerate-people",
            axum::routing::post(regenerate_people),
        )
}

async fn list_my_requests(State(app): State<App>, request: Request) -> Response {
    let (headers, raw_query) = session_parts(&request);
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    match app.list_my_cluster_requests(&user_id).await {
        Ok(body) => json_ok(body),
        Err(error) => cluster_error(error),
    }
}

async fn list_group_requests(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (headers, raw_query) = session_parts(&request);
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.list_group_requests(&user_id, &id).await {
        Ok(body) => json_ok(body),
        Err(error) => cluster_error(error),
    }
}

async fn list_group_users(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (headers, raw_query) = session_parts(&request);
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.list_group_users(&user_id, &id).await {
        Ok(body) => json_ok(body),
        Err(error) => cluster_error(error),
    }
}

async fn create_request(
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
    match app.create_cluster_request(&user_id, &id, &body).await {
        Ok(stored) => {
            if stored.created {
                (StatusCode::CREATED, axum::Json(stored.document)).into_response()
            } else {
                json_ok(stored.document)
            }
        }
        Err(error) => cluster_error(error),
    }
}

async fn accept_request(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (headers, raw_query) = session_parts(&request);
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.accept_cluster_request(&user_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => cluster_error(error),
    }
}

async fn delete_request(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (headers, raw_query) = session_parts(&request);
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.delete_cluster_request(&user_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => cluster_error(error),
    }
}

async fn leave_group(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let (headers, raw_query) = session_parts(&request);
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.leave_cluster_group(&user_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => cluster_error(error),
    }
}

async fn regenerate_people(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let (headers, raw_query) = session_parts(&request);
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.regenerate_cluster_people(&user_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => cluster_error(error),
    }
}

fn cluster_error(error: ClusterFail) -> Response {
    match error {
        ClusterFail::BadRequest(text) => message(StatusCode::BAD_REQUEST, text),
        ClusterFail::Db(error) => db_error(error),
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
