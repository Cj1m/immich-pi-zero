//! Workflow HTTP. A share key or a shared-link cookie is 403. The access-token cookie is the session.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::Value;

use crate::db::App;
use crate::http::{db_error, json_ok, message};
use crate::http_duplicates::signed_in;
use crate::workflows::{self, WorkflowFail};

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/workflows/triggers", get(triggers))
        .route("/api/workflows", get(list_workflows).post(create_workflow))
        .route("/api/workflows/{id}/share", get(share_workflow))
        .route("/api/workflows/{id}/logs", get(workflow_logs))
        .route(
            "/api/workflows/{id}",
            get(get_workflow)
                .put(update_workflow)
                .patch(update_workflow)
                .delete(delete_workflow),
        )
}

async fn triggers(State(app): State<App>, request: Request) -> Response {
    let (headers, raw_query) = session_parts(&request);
    if let Err(response) = signed_in(&app, &headers, &raw_query).await {
        return response;
    }
    json_ok(app.workflow_triggers())
}

async fn list_workflows(State(app): State<App>, request: Request) -> Response {
    let (headers, raw_query) = session_parts(&request);
    let user_id = match signed_in(&app, &headers, &raw_query).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let query = match workflows::workflow_query(&raw_query) {
        Ok(query) => query,
        Err(error) => return workflow_error(error),
    };
    match app.list_workflows(&user_id, &query).await {
        Ok(body) => json_ok(body),
        Err(error) => workflow_error(error),
    }
}

async fn create_workflow(State(app): State<App>, request: Request) -> Response {
    let (user_id, body) = match json_body(&app, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match app.create_workflow(&user_id, &body).await {
        Ok(body) => (StatusCode::CREATED, axum::Json(body)).into_response(),
        Err(error) => workflow_error(error),
    }
}

async fn get_workflow(
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
    match app.get_workflow(&user_id, &id).await {
        Ok(body) => json_ok(body),
        Err(error) => workflow_error(error),
    }
}

async fn share_workflow(
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
    match app.share_workflow(&user_id, &id).await {
        Ok(body) => json_ok(body),
        Err(error) => workflow_error(error),
    }
}

async fn update_workflow(
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
    match app.update_workflow(&user_id, &id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => workflow_error(error),
    }
}

async fn delete_workflow(
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
    match app.delete_workflow(&user_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => workflow_error(error),
    }
}

async fn workflow_logs(
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
    let query = match workflows::workflow_query(&raw_query) {
        Ok(query) => query,
        Err(error) => return workflow_error(error),
    };
    match app.workflow_logs(&user_id, &id, &query).await {
        Ok(body) => json_ok(body),
        Err(error) => workflow_error(error),
    }
}

fn workflow_error(error: WorkflowFail) -> Response {
    match error {
        WorkflowFail::BadRequest(text) => message(StatusCode::BAD_REQUEST, text),
        WorkflowFail::Db(error) => db_error(error),
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
