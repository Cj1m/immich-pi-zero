//! Smart search. Text with no stored vectors is an empty page. A photo with no vector is 400.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use serde_json::{json, Value};

use crate::db::App;
use crate::http::{db_error, json_ok, message, require_session};
use crate::ml::{self, smart_search_enabled};
use crate::system_config::clip_dimensions;

pub(crate) fn routes() -> Router<App> {
    Router::new().route("/api/search/smart", post(search_smart))
}

async fn search_smart(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024 * 1024).await else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    let Some(page) = page_number(&body) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let Some(size) = page_size(&body) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let config = match app.admin_config().await {
        Ok(config) => config,
        Err(error) => return db_error(error),
    };
    if !smart_search_enabled(&config) {
        return message(StatusCode::BAD_REQUEST, "Smart search is not enabled");
    }
    let query = body["query"].as_str().map(str::trim).filter(|query| !query.is_empty());
    let query_asset = body["queryAssetId"].as_str().map(str::trim).filter(|id| !id.is_empty());
    let embedding = if let Some(query) = query {
        let model = config["machineLearning"]["clip"]["modelName"]
            .as_str()
            .unwrap_or("");
        let Some(dim) = clip_dimensions(model) else {
            return empty_page();
        };
        let language = body["language"].as_str().filter(|value| !value.is_empty());
        match ml::encode_text(&config, model, query, language).await {
            Ok(embedding) if embedding.len() == dim => embedding,
            Ok(_) => return empty_page(),
            Err(error) => {
                eprintln!("clip text: {error}");
                return message(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error");
            }
        }
    } else if let Some(asset_id) = query_asset {
        match ml::asset_embedding(&app, &session.user.id, asset_id).await {
            Ok(None) => {
                return message(StatusCode::BAD_REQUEST, "Not found or no asset.read access");
            }
            Ok(Some(embedding)) if embedding.is_empty() => {
                return message(
                    StatusCode::BAD_REQUEST,
                    format!("Asset {asset_id} has no embedding"),
                );
            }
            Ok(Some(embedding)) => embedding,
            Err(error) => return db_error(error),
        }
    } else {
        return message(
            StatusCode::BAD_REQUEST,
            "Either `query` or `queryAssetId` must be set",
        );
    };
    let hits = match ml::search_hits(&app, &session.user.id, &embedding).await {
        Ok(hits) => hits,
        Err(error) => return db_error(error),
    };
    let start = (page - 1).saturating_mul(size);
    let page_hits: Vec<_> = hits.into_iter().skip(start).take(size + 1).collect();
    let has_next = page_hits.len() > size;
    let mut items = Vec::new();
    for (id, _) in page_hits.into_iter().take(size) {
        match app.asset_json(&session.user.id, &id).await {
            Ok(asset) => items.push(asset),
            Err(error) => return db_error(error),
        }
    }
    let next_page = has_next.then(|| (page + 1).to_string());
    json_ok(search_body(items, next_page))
}

fn page_number(body: &Value) -> Option<usize> {
    match body.get("page") {
        None | Some(Value::Null) => Some(1),
        Some(value) => match value.as_u64() {
            Some(page) if (1..=10_000).contains(&page) => Some(page as usize),
            _ => None,
        },
    }
}

fn page_size(body: &Value) -> Option<usize> {
    match body.get("size") {
        None | Some(Value::Null) => Some(100),
        Some(value) => match value.as_u64() {
            Some(size) if (1..=1000).contains(&size) => Some(size as usize),
            _ => None,
        },
    }
}

fn empty_page() -> Response {
    json_ok(search_body(Vec::new(), None))
}

fn search_body(items: Vec<Value>, next_page: Option<String>) -> Value {
    let count = items.len();
    json!({
        "albums": { "total": 0, "count": 0, "items": [], "facets": [] },
        "assets": {
            "total": count,
            "count": count,
            "items": items,
            "facets": [],
            "nextPage": next_page,
            "nextCursor": null,
        }
    })
}
