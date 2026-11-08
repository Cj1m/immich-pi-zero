//! Plugin HTTP. A share key or a shared-link cookie is 403. The access-token cookie is the session.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;

use crate::db::App;
use crate::http::{db_error, json_ok, message};
use crate::http_duplicates::signed_in;

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/plugins/methods", get(plugin_methods))
        .route("/api/plugins/templates", get(plugin_templates))
        .route("/api/plugins", get(list_plugins))
        .route("/api/plugins/{id}", get(get_plugin))
}

async fn plugin_methods(State(app): State<App>, request: Request) -> Response {
    let (headers, raw_query) = session_parts(&request);
    if let Err(response) = signed_in(&app, &headers, &raw_query).await {
        return response;
    }
    let trigger = query_value(&raw_query, "trigger");
    match app.plugin_methods(trigger.as_deref()).await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn plugin_templates(State(app): State<App>, request: Request) -> Response {
    let (headers, raw_query) = session_parts(&request);
    if let Err(response) = signed_in(&app, &headers, &raw_query).await {
        return response;
    }
    match app.plugin_templates().await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn list_plugins(State(app): State<App>, request: Request) -> Response {
    let (headers, raw_query) = session_parts(&request);
    if let Err(response) = signed_in(&app, &headers, &raw_query).await {
        return response;
    }
    match app.list_plugins().await {
        Ok(body) => json_ok(body),
        Err(error) => db_error(error),
    }
}

async fn get_plugin(State(app): State<App>, Path(id): Path<String>, request: Request) -> Response {
    let (headers, raw_query) = session_parts(&request);
    if let Err(response) = signed_in(&app, &headers, &raw_query).await {
        return response;
    }
    if !is_uuid_v4(&id) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    match app.plugin_document(&id).await {
        Ok(Some(body)) => json_ok(body),
        Ok(None) => message(StatusCode::BAD_REQUEST, "Plugin not found"),
        Err(error) => db_error(error),
    }
}

fn session_parts(request: &Request) -> (HeaderMap, String) {
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    (headers, raw_query)
}

fn query_value(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|item| {
        let (key, value) = item.split_once('=').unwrap_or((item, ""));
        if key == name && !value.is_empty() {
            Some(value.to_string())
        } else {
            None
        }
    })
}

fn is_uuid_v4(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.get_version() == Some(uuid::Version::Random))
}
