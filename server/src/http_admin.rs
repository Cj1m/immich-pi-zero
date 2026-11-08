//! Admin queues, jobs, maintenance, backups, integrity, license, and server statistics.
//! Maintenance status and maintenance login are public. The other routes need an admin,
//! except the signed-in user's own license and `GET /api/server/about`.

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde_json::Value;

use crate::admin::{AdminFail, MaintenanceOutcome};
use crate::db::{App, CurrentSession};
use crate::http::{db_error, json_ok, message, request_is_secure, require_session};

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/queues", get(queues))
        .route(
            "/api/queues/{name}/jobs",
            get(queue_jobs).delete(empty_queue),
        )
        .route("/api/queues/{name}", get(queue).put(update_queue))
        .route("/api/jobs", get(legacy_queues).post(create_job))
        .route("/api/jobs/{name}", put(queue_command))
        .route("/api/admin/maintenance/status", get(maintenance_status))
        .route("/api/admin/maintenance/login", post(maintenance_login))
        .route("/api/admin/maintenance", post(set_maintenance))
        .route(
            "/api/admin/database-backups",
            get(list_backups).delete(delete_backups),
        )
        .route(
            "/api/admin/database-backups/{filename}",
            get(download_backup),
        )
        .route("/api/admin/integrity/summary", get(integrity_summary))
        .route("/api/admin/integrity/report", get(integrity_report))
        .route("/api/admin/integrity/report/{kind}/csv", get(integrity_csv))
        .route(
            "/api/admin/notifications/templates/{name}",
            post(email_template),
        )
        .route("/api/server/about", get(about))
        .route("/api/server/statistics", get(statistics))
        .route(
            "/api/server/license",
            get(server_license)
                .put(set_server_license)
                .delete(delete_server_license),
        )
        .route(
            "/api/users/me/license",
            get(user_license)
                .put(set_user_license)
                .delete(delete_user_license),
        )
}

fn admin_error(error: AdminFail) -> Response {
    match error {
        AdminFail::Db(error) => db_error(error),
        AdminFail::BadRequest(text) => message(StatusCode::BAD_REQUEST, text),
        AdminFail::NotFound => message(StatusCode::NOT_FOUND, "Not found"),
        AdminFail::Unauthorized => message(StatusCode::UNAUTHORIZED, "Unauthorized"),
    }
}

async fn admin_user(app: &App, headers: &HeaderMap) -> Result<CurrentSession, Response> {
    let session = require_session(app, headers).await?;
    if session.user.is_admin {
        Ok(session)
    } else {
        Err(message(StatusCode::FORBIDDEN, "Forbidden"))
    }
}

async fn read_json(request: Request) -> Result<(HeaderMap, String, Value, bool), Response> {
    let secure = request_is_secure(&request);
    let (parts, body) = request.into_parts();
    let query = parts.uri.query().unwrap_or("").to_string();
    let headers = parts.headers;
    let bytes = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .map_err(|_| message(StatusCode::BAD_REQUEST, "Validation failed"))?;
    if bytes.is_empty() {
        return Ok((headers, query, Value::Null, secure));
    }
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => Ok((headers, query, value, secure)),
        Err(_) => Err(message(StatusCode::BAD_REQUEST, "Validation failed")),
    }
}

fn query_values(query: &str, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    for pair in query.split('&') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name == key {
            values.push(value.to_string());
        }
    }
    values
}

fn query_one(query: &str, key: &str) -> Option<String> {
    query_values(query, key).into_iter().next()
}

fn maintenance_cookie(secret: &str, secure: bool) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    format!("immich_maintenance_token={secret}; Path=/; SameSite=Lax; HttpOnly{secure_flag}")
}

fn clear_maintenance_cookie() -> String {
    "immich_maintenance_token=; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT"
        .to_string()
}

async fn queues(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.queue_list().await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn queue(State(app): State<App>, Path(name): Path<String>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.queue_one(&name).await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn update_queue(State(app): State<App>, request: Request) -> Response {
    let name = request
        .uri()
        .path()
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    let (headers, _, body, _) = match read_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.update_queue(&name, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn queue_jobs(State(app): State<App>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let name = path
        .trim_end_matches("/jobs")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    let query = request.uri().query().unwrap_or("").to_string();
    let headers = request.headers().clone();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    let statuses = query_values(&query, "status");
    match app.queue_jobs(&name, &statuses).await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn empty_queue(State(app): State<App>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let name = path
        .trim_end_matches("/jobs")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    let (headers, _, body, _) = match read_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.empty_queue_jobs(&name, &body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => admin_error(error),
    }
}

async fn legacy_queues(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.legacy_queues().await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn create_job(State(app): State<App>, request: Request) -> Response {
    let (headers, _, body, _) = match read_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.create_manual_job(&body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => admin_error(error),
    }
}

async fn queue_command(
    State(app): State<App>,
    Path(name): Path<String>,
    request: Request,
) -> Response {
    let (headers, _, body, _) = match read_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.queue_command(&name, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn maintenance_status(State(app): State<App>) -> Response {
    match app.maintenance_status().await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn maintenance_login(State(app): State<App>, request: Request) -> Response {
    let (headers, _, body, secure) = match read_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let token = body
        .get("token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| crate::http::cookie(&headers, "immich_maintenance_token"));
    match app.maintenance_login(token.as_deref()).await {
        Ok(document) => {
            let mut response = json_ok(document);
            if let Some(token) = token {
                if let Ok(value) = maintenance_cookie(&token, secure).parse() {
                    response.headers_mut().insert(header::SET_COOKIE, value);
                }
            }
            response
        }
        Err(error) => admin_error(error),
    }
}

async fn set_maintenance(State(app): State<App>, request: Request) -> Response {
    let (headers, _, body, secure) = match read_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session = match admin_user(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.set_maintenance(&session.user.name, &body).await {
        Ok(MaintenanceOutcome::Unchanged) => StatusCode::OK.into_response(),
        Ok(MaintenanceOutcome::Ended) => {
            let mut response = StatusCode::OK.into_response();
            if let Ok(value) = clear_maintenance_cookie().parse() {
                response.headers_mut().insert(header::SET_COOKIE, value);
            }
            response
        }
        Ok(MaintenanceOutcome::Entered(secret)) => {
            let mut response = StatusCode::OK.into_response();
            if let Ok(value) = maintenance_cookie(&secret, secure).parse() {
                response.headers_mut().insert(header::SET_COOKIE, value);
            }
            response
        }
        Err(error) => admin_error(error),
    }
}

async fn list_backups(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.list_backups().await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn delete_backups(State(app): State<App>, request: Request) -> Response {
    let (headers, _, body, _) = match read_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.delete_backups(&body).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(error) => admin_error(error),
    }
}

async fn download_backup(
    State(app): State<App>,
    Path(filename): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.read_backup(&filename).await {
        Ok((bytes, content_type)) => {
            let length = bytes.len();
            let mut response = (StatusCode::OK, bytes).into_response();
            let headers = response.headers_mut();
            if let Ok(value) = content_type.parse() {
                headers.insert(header::CONTENT_TYPE, value);
            }
            if let Ok(value) = length.to_string().parse() {
                headers.insert(header::CONTENT_LENGTH, value);
            }
            response
        }
        Err(error) => admin_error(error),
    }
}

async fn integrity_summary(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.integrity_summary().await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn integrity_report(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    let Some(kind) = query_one(&query, "type") else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let limit = match query_one(&query, "limit") {
        None => 500,
        Some(text) => match text.parse::<i64>() {
            Ok(value) if value > 0 => value,
            _ => return message(StatusCode::BAD_REQUEST, "Validation failed"),
        },
    };
    let cursor = query_one(&query, "cursor");
    match app.integrity_report(&kind, cursor.as_deref(), limit).await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn integrity_csv(
    State(app): State<App>,
    Path(kind): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.integrity_csv(&kind).await {
        Ok(csv) => {
            let mut response = (StatusCode::OK, csv).into_response();
            if let Ok(value) = "text/csv".parse() {
                response.headers_mut().insert(header::CONTENT_TYPE, value);
            }
            response
        }
        Err(error) => admin_error(error),
    }
}

async fn email_template(
    State(app): State<App>,
    Path(name): Path<String>,
    request: Request,
) -> Response {
    let (headers, _, body, _) = match read_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match App::email_template(&name, &body) {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn about(State(app): State<App>, request: Request) -> Response {
    if let Err(response) = require_session(&app, request.headers()).await {
        return response;
    }
    match app.server_about().await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn statistics(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.server_statistics().await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn server_license(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.get_server_license().await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn set_server_license(State(app): State<App>, request: Request) -> Response {
    let (headers, _, body, _) = match read_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.set_server_license(&body).await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn delete_server_license(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Err(response) = admin_user(&app, &headers).await {
        return response;
    }
    match app.delete_server_license().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => admin_error(error),
    }
}

async fn user_license(State(app): State<App>, request: Request) -> Response {
    let session = match require_session(&app, request.headers()).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.get_user_license(&session.user.id).await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn set_user_license(State(app): State<App>, request: Request) -> Response {
    let (headers, _, body, _) = match read_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session = match require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.set_user_license(&session.user.id, &body).await {
        Ok(body) => json_ok(body),
        Err(error) => admin_error(error),
    }
}

async fn delete_user_license(State(app): State<App>, request: Request) -> Response {
    let session = match require_session(&app, request.headers()).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.delete_user_license(&session.user.id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => admin_error(error),
    }
}
