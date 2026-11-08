//! PIN and locked-folder session HTTP. A share key or shared-link cookie is 403.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::Value;

use crate::db::{App, CurrentSession};
use crate::http::{db_error, json_ok, message};
use crate::http_account::account_caller;
use crate::pin::PinFail;

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/auth/status", get(auth_status))
        .route(
            "/api/auth/pin-code",
            axum::routing::post(setup_pin)
                .put(change_pin)
                .delete(reset_pin),
        )
        .route(
            "/api/auth/session/unlock",
            axum::routing::post(unlock_session),
        )
        .route("/api/auth/session/lock", axum::routing::post(lock_session))
}

async fn auth_status(State(app): State<App>, request: Request) -> Response {
    let (headers, query) = parts(&request);
    let caller = match account_caller(&app, &headers, &query, "all").await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let session = match reload_session(&app, &headers, caller.session_id.as_deref()).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match app.auth_status(&caller.user_id, session.as_ref()).await {
        Ok(body) => json_ok(body),
        Err(error) => pin_error(error),
    }
}

async fn setup_pin(State(app): State<App>, request: Request) -> Response {
    let (caller, body) = match caller_body(&app, request, "pinCode.create").await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(Some(pin)) = pin_digits(body.get("pinCode"), true) else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    match app.setup_pin(&caller.user_id, &pin).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => pin_error(error),
    }
}

async fn change_pin(State(app): State<App>, request: Request) -> Response {
    let (caller, body) = match caller_body(&app, request, "pinCode.update").await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (password, pin, new_pin) = match change_fields(&body) {
        Ok(value) => value,
        Err(()) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    match app
        .change_pin(
            &caller.user_id,
            password.as_deref(),
            pin.as_deref(),
            &new_pin,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => pin_error(error),
    }
}

async fn reset_pin(State(app): State<App>, request: Request) -> Response {
    let (caller, body) = match caller_body(&app, request, "pinCode.delete").await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (password, pin) = match secret_fields(&body) {
        Ok(value) => value,
        Err(()) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    match app
        .reset_pin(&caller.user_id, password.as_deref(), pin.as_deref())
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => pin_error(error),
    }
}

async fn unlock_session(State(app): State<App>, request: Request) -> Response {
    let (caller, body) = match caller_body(&app, request, "all").await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(session_id) = caller.session_id else {
        return message(
            StatusCode::BAD_REQUEST,
            "This endpoint can only be used with a session token",
        );
    };
    let pin = match optional_pin(body.get("pinCode")) {
        Ok(pin) => pin,
        Err(()) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    match app
        .unlock_session(&caller.user_id, &session_id, pin.as_deref())
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => pin_error(error),
    }
}

async fn lock_session(State(app): State<App>, request: Request) -> Response {
    let (headers, query) = parts(&request);
    let caller = match account_caller(&app, &headers, &query, "all").await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let Some(session_id) = caller.session_id else {
        return message(
            StatusCode::BAD_REQUEST,
            "This endpoint can only be used with a session token",
        );
    };
    match app.lock_session(&session_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => pin_error(error),
    }
}

async fn reload_session(
    app: &App,
    headers: &HeaderMap,
    session_id: Option<&str>,
) -> Result<Option<CurrentSession>, Response> {
    let Some(_) = session_id else {
        return Ok(None);
    };
    let Some(token) = crate::http::cookie(headers, "immich_access_token") else {
        return Ok(None);
    };
    app.session_user(&token).await.map(Some).map_err(db_error)
}

async fn caller_body(
    app: &App,
    request: Request,
    permission: &str,
) -> Result<(crate::http_account::Caller, Value), Response> {
    let (headers, query) = parts(&request);
    let caller = account_caller(app, &headers, &query, permission).await?;
    let bytes = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .map_err(|_| message(StatusCode::BAD_REQUEST, "Validation failed"))?;
    let body = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) if value.is_object() => value,
        _ => return Err(message(StatusCode::BAD_REQUEST, "Validation failed")),
    };
    Ok((caller, body))
}

fn parts(request: &Request) -> (HeaderMap, String) {
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    (headers, query)
}

fn change_fields(body: &Value) -> Result<(Option<String>, Option<String>, String), ()> {
    let (password, pin) = secret_fields(body)?;
    let Some(new_pin) = pin_digits(body.get("newPinCode"), true)? else {
        return Err(());
    };
    Ok((password, pin, new_pin))
}

fn secret_fields(body: &Value) -> Result<(Option<String>, Option<String>), ()> {
    let password = match body.get("password") {
        None => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Err(()),
    };
    let pin = optional_pin(body.get("pinCode"))?;
    Ok((password, pin))
}

fn optional_pin(value: Option<&Value>) -> Result<Option<String>, ()> {
    pin_digits(value, false)
}

fn pin_digits(value: Option<&Value>, required: bool) -> Result<Option<String>, ()> {
    match value {
        None if !required => Ok(None),
        Some(Value::String(text)) if is_pin(text) => Ok(Some(text.clone())),
        _ => Err(()),
    }
}

fn is_pin(text: &str) -> bool {
    text.len() == 6 && text.chars().all(|char| char.is_ascii_digit())
}

fn pin_error(error: PinFail) -> Response {
    match error {
        PinFail::Db(error) => db_error(error),
        PinFail::BadRequest(text) => message(StatusCode::BAD_REQUEST, text),
        PinFail::Unauthorized(text) => message(StatusCode::UNAUTHORIZED, text),
    }
}
