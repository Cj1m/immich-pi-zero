//! OAuth HTTP. The four login and link calls are public or signed-in the way v3.3.6 is.

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

use crate::db::{App, DbError};
use crate::http::{
    append_cookie, cookie, db_error, login_response, message, user_json, COOKIE_MAX_AGE_SECONDS,
};
use crate::oauth::OAuthFail;

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/oauth/mobile-redirect", get(mobile_redirect))
        .route("/api/oauth/authorize", post(authorize))
        .route("/api/oauth/callback", post(callback))
        .route("/api/oauth/link", post(link))
        .route("/api/oauth/unlink", post(unlink))
        .route("/api/oauth/backchannel-logout", post(backchannel))
}

pub(crate) fn oauth_error(error: OAuthFail) -> Response {
    match error {
        OAuthFail::BadRequest(text) => message(StatusCode::BAD_REQUEST, text),
        OAuthFail::Discovery(text) => message(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Error in OAuth discovery: {text}"),
        ),
        OAuthFail::LoginFailed => {
            message(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        }
        OAuthFail::Db(DbError::Database(error)) => {
            eprintln!("database: {error}");
            message(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        }
        OAuthFail::Db(error) => db_error(error),
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

fn secure_cookie(value: &str, secure: bool, http_only: bool) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    let http_only = if http_only { "; HttpOnly" } else { "" };
    format!(
        "{value}; Max-Age={COOKIE_MAX_AGE_SECONDS}; Path=/; SameSite=Lax{secure_flag}{http_only}"
    )
}

fn clear_cookie(name: &str) -> String {
    format!("{name}=; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT")
}

fn optional_string(body: &Value, key: &str) -> Result<Option<String>, Box<Response>> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(Box::new(message(
            StatusCode::BAD_REQUEST,
            "Validation failed",
        ))),
    }
}

async fn mobile_redirect(request: Request) -> Response {
    let target = crate::db::App::mobile_oauth_redirect(&request.uri().to_string());
    (
        StatusCode::TEMPORARY_REDIRECT,
        [(header::LOCATION, target)],
        "",
    )
        .into_response()
}

async fn authorize(State(app): State<App>, request: Request) -> Response {
    let secure = crate::http::request_is_secure(&request);
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(object) = body.as_object() else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let Some(redirect_uri) = object.get("redirectUri").and_then(Value::as_str) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let state = match optional_string(&body, "state") {
        Ok(state) => state,
        Err(response) => return *response,
    };
    let challenge = match optional_string(&body, "codeChallenge") {
        Ok(challenge) => challenge,
        Err(response) => return *response,
    };
    let started = match app
        .oauth_authorize(redirect_uri, state.as_deref(), challenge.as_deref())
        .await
    {
        Ok(started) => started,
        Err(error) => return oauth_error(error),
    };
    let mut response = (
        StatusCode::CREATED,
        axum::Json(json!({ "url": started.url })),
    )
        .into_response();
    let headers = response.headers_mut();
    append_cookie(
        headers,
        secure_cookie(
            &format!("immich_oauth_state={}", started.state),
            secure,
            true,
        ),
    );
    let verifier = started.code_verifier.unwrap_or_default();
    append_cookie(
        headers,
        secure_cookie(
            &format!("immich_oauth_code_verifier={verifier}"),
            secure,
            true,
        ),
    );
    response
}

async fn callback(State(app): State<App>, request: Request) -> Response {
    let secure = crate::http::request_is_secure(&request);
    let headers = request.headers().clone();
    let device = crate::http::client_device(&headers);
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(url) = callback_url(&body) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let state = match secret_from(&body, &headers, "state", "immich_oauth_state") {
        Ok(state) => state,
        Err(response) => return *response,
    };
    let verifier = match secret_from(
        &body,
        &headers,
        "codeVerifier",
        "immich_oauth_code_verifier",
    ) {
        Ok(verifier) => verifier,
        Err(response) => return *response,
    };
    let success = match app
        .oauth_callback(&url, state.as_deref(), verifier.as_deref(), &device)
        .await
    {
        Ok(success) => success,
        Err(error) => return oauth_error(error),
    };
    let mut response = login_response(success.token, &success.user, secure, "oauth");
    let headers = response.headers_mut();
    append_cookie(headers, clear_cookie("immich_oauth_state"));
    append_cookie(headers, clear_cookie("immich_oauth_code_verifier"));
    response
}

async fn link(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let session = match crate::http_duplicates::signed_in(
        &app,
        &headers,
        request.uri().query().unwrap_or(""),
    )
    .await
    {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let session_id = match crate::http::require_session(&app, &headers).await {
        Ok(session) => session.session_id,
        Err(response) => return response,
    };
    let body = match read_json(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(url) = callback_url(&body) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let state = match secret_from(&body, &headers, "state", "immich_oauth_state") {
        Ok(state) => state,
        Err(response) => return *response,
    };
    let verifier = match secret_from(
        &body,
        &headers,
        "codeVerifier",
        "immich_oauth_code_verifier",
    ) {
        Ok(verifier) => verifier,
        Err(response) => return *response,
    };
    match app
        .oauth_link(
            &session,
            &session_id,
            &url,
            state.as_deref(),
            verifier.as_deref(),
        )
        .await
    {
        Ok(user) => user_response(&app, user).await,
        Err(error) => oauth_error(error),
    }
}

async fn unlink(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    let user_id = match crate::http_duplicates::signed_in(
        &app,
        &headers,
        request.uri().query().unwrap_or(""),
    )
    .await
    {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let session = match crate::http::require_session(&app, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.oauth_unlink(&user_id, &session.session_id).await {
        Ok(user) => user_response(&app, user).await,
        Err(error) => oauth_error(error),
    }
}

async fn backchannel(State(app): State<App>, request: Request) -> Response {
    let bytes = match axum::body::to_bytes(request.into_body(), 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let Some(token) = form_value(text, "logout_token").filter(|token| !token.is_empty()) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    match app.oauth_backchannel(&token).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(error) => oauth_error(error),
    }
}

async fn user_response(app: &App, user: crate::db::UserDocument) -> Response {
    let mut body = user_json(&user);
    match app.user_license_value(&user.id).await {
        Ok(license) => body["license"] = license,
        Err(error) => return db_error(error),
    }
    (StatusCode::OK, axum::Json(body)).into_response()
}

fn callback_url(body: &Value) -> Option<String> {
    let url = body.get("url")?.as_str()?;
    if url.is_empty() {
        None
    } else {
        Some(url.to_string())
    }
}

fn secret_from(
    body: &Value,
    headers: &HeaderMap,
    key: &str,
    cookie_name: &str,
) -> Result<Option<String>, Box<Response>> {
    match optional_string(body, key)? {
        Some(value) => Ok(Some(value)),
        None => Ok(cookie(headers, cookie_name)),
    }
}

fn form_value(body: &str, name: &str) -> Option<String> {
    for pair in body.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(key) == name {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_decode(value: &str) -> String {
    let mut bytes = Vec::new();
    let input = value.as_bytes();
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' && index + 2 < input.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                bytes.push(byte);
                index += 3;
                continue;
            }
        }
        if input[index] == b'+' {
            bytes.push(b' ');
        } else {
            bytes.push(input[index]);
        }
        index += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
