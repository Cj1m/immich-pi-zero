//! OAuth matches v3.3.6 when it is enabled. Password login stays the unconfigured path.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use base64::Engine;
use http::Request;
use http_body_util::BodyExt;
use immich_server::{App, Config};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

fn config(dir: &Path) -> Config {
    Config {
        data_dir: dir.to_path_buf(),
        host: std::net::IpAddr::from([127, 0, 0, 1]),
        port: 0,
        admin_email: Some("admin@example.com".to_string()),
        admin_name: Some("Ada".to_string()),
        admin_password: Some("correct horse".to_string()),
        s3: None,
        web_root: dir.join("web"),
        machine_learning_url: "http://immich-machine-learning:3003".to_string(),
        machine_learning_enabled: false,
        config_file: None,
        log_level: None,
    }
}

struct Reply {
    status: http::StatusCode,
    body: Vec<u8>,
    headers: http::HeaderMap,
}

async fn call(app: &App, request: Request<Body>) -> Reply {
    let response = immich_server::router(app.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Reply {
        status,
        body,
        headers,
    }
}

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| panic!("body {}", String::from_utf8_lossy(bytes)))
}

fn message(bytes: &[u8]) -> String {
    json_body(bytes)["message"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn json_request(
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        return builder.body(Body::from(body.to_string())).unwrap();
    }
    builder.body(Body::empty()).unwrap()
}

fn cookie_pair(headers: &http::HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    headers.get_all("set-cookie").iter().find_map(|value| {
        let text = value.to_str().ok()?;
        let pair = text.split(';').next()?;
        pair.starts_with(&prefix).then(|| pair.to_string())
    })
}

fn cookie_header(headers: &http::HeaderMap, name: &str) -> Option<String> {
    headers.get_all("set-cookie").iter().find_map(|value| {
        let text = value.to_str().ok()?;
        text.split(';')
            .next()?
            .starts_with(&format!("{name}="))
            .then(|| text.to_string())
    })
}

async fn sign_in(app: &App, email: &str, password: &str) -> String {
    let reply = call(
        app,
        json_request(
            "POST",
            "/api/auth/login",
            None,
            Some(&format!(r#"{{"email":"{email}","password":"{password}"}}"#)),
        ),
    )
    .await;
    assert_eq!(reply.status, 201, "{}", message(&reply.body));
    cookie_pair(&reply.headers, "immich_access_token").unwrap()
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut key_block = [0_u8; 64];
    if key.len() > 64 {
        key_block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = [0_u8; 64];
    let mut outer = [0_u8; 64];
    for index in 0..64 {
        inner[index] = key_block[index] ^ 0x36;
        outer[index] = key_block[index] ^ 0x5c;
    }
    let mut inner_data = inner.to_vec();
    inner_data.extend_from_slice(message);
    let inner_hash = Sha256::digest(&inner_data);
    let mut outer_data = outer.to_vec();
    outer_data.extend_from_slice(&inner_hash);
    Sha256::digest(&outer_data).into()
}

fn sign_hs256(secret: &str, payload: &Value) -> String {
    let header = b64url(br#"{"alg":"HS256","typ":"JWT"}"#);
    let body = b64url(serde_json::to_string(payload).unwrap().as_bytes());
    let input = format!("{header}.{body}");
    let signature = hmac_sha256(secret.as_bytes(), input.as_bytes());
    format!("{input}.{}", b64url(&signature))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn token_for(
    origin: &str,
    secret: &str,
    sub: &str,
    email: Option<&str>,
    sid: &str,
    extra: Value,
) -> String {
    let mut payload = json!({
        "iss": origin,
        "aud": "immich-client",
        "sub": sub,
        "sid": sid,
        "iat": unix_now(),
        "exp": unix_now() + 600,
    });
    if let Some(email) = email {
        payload["email"] = json!(email);
    }
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            payload[key] = value.clone();
        }
    }
    sign_hs256(secret, &payload)
}

fn form_value(body: &str, name: &str) -> String {
    for pair in body.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == name {
            return value.to_string();
        }
    }
    String::new()
}

fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 2048];
    let header_end;
    loop {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = position + 4;
            break;
        }
    }
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let request = head.lines().next()?.to_string();
    Some((request, buffer[header_end..].to_vec()))
}

fn write_json(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn spawn_issuer(origin: String, secret: String) {
    let listener = TcpListener::bind(origin.trim_start_matches("http://")).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some((request, body)) = read_request(&mut stream) else {
                continue;
            };
            let path = request.split_whitespace().nth(1).unwrap_or("/");
            let path = path.split('?').next().unwrap_or(path);
            let text = String::from_utf8_lossy(&body);
            let response = if path.ends_with("/.well-known/openid-configuration") {
                json!({
                    "issuer": origin,
                    "authorization_endpoint": format!("{origin}/authorize"),
                    "token_endpoint": format!("{origin}/token"),
                    "userinfo_endpoint": format!("{origin}/userinfo"),
                    "jwks_uri": format!("{origin}/jwks"),
                    "end_session_endpoint": format!("{origin}/end"),
                    "code_challenge_methods_supported": ["S256"]
                })
                .to_string()
            } else if path.ends_with("/token") {
                let code = form_value(&text, "code");
                let (sub, email, sid, extra) = match code.as_str() {
                    "new" => (
                        "new-sub",
                        Some("new-person@example.com"),
                        "sid-new",
                        json!({"name": "New Person", "immich_role": "admin", "preferred_username": "newperson", "immich_quota": 1}),
                    ),
                    "no-email" => ("bare-sub", None, "sid-bare", json!({})),
                    "unknown" => (
                        "unknown-sub",
                        Some("nobody@example.com"),
                        "sid-unknown",
                        json!({}),
                    ),
                    "conflict" => (
                        "other-sub",
                        Some("cam@example.com"),
                        "sid-conflict",
                        json!({}),
                    ),
                    "link-guest" => (
                        "guest-oauth",
                        Some("bea@example.com"),
                        "sid-guest",
                        json!({}),
                    ),
                    "link-admin" => (
                        "admin-sub",
                        Some("admin@example.com"),
                        "sid-link",
                        json!({}),
                    ),
                    _ => (
                        "admin-sub",
                        Some("admin@example.com"),
                        "sid-admin",
                        json!({}),
                    ),
                };
                let id_token = token_for(&origin, &secret, sub, email, sid, extra);
                json!({"access_token": "access-token", "token_type": "Bearer", "id_token": id_token}).to_string()
            } else if path.ends_with("/userinfo") {
                json!({"sub": "bare-sub"}).to_string()
            } else {
                json!({}).to_string()
            };
            write_json(&mut stream, &response);
        }
    });
}

async fn save_config(app: &App, admin: &str, edit: impl FnOnce(&mut Value)) {
    let reply = call(
        app,
        json_request("GET", "/api/system-config", Some(admin), None),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", message(&reply.body));
    let mut document = json_body(&reply.body);
    edit(&mut document);
    let reply = call(
        app,
        json_request(
            "PUT",
            "/api/system-config",
            Some(admin),
            Some(&document.to_string()),
        ),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", message(&reply.body));
}

#[tokio::test]
async fn oauth_login_links_a_user_and_logs_out() {
    let dir = std::env::temp_dir().join(format!("immich-oauth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let origin = format!("http://127.0.0.1:{port}");
    let secret = "top-secret";
    spawn_issuer(origin.clone(), secret.to_string());

    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let admin = sign_in(&app, "admin@example.com", "correct horse").await;

    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/authorize",
            None,
            Some(r#"{"redirectUri":"https://photos.example/auth/login"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "OAuth is not enabled");

    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/callback",
            None,
            Some(r#"{"url":"https://photos.example/auth/login?code=1&state=abc"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "OAuth is not enabled");

    let reply = call(
        &app,
        json_request("POST", "/api/oauth/backchannel-logout", None, None),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Validation failed");

    let reply = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/oauth/backchannel-logout")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("logout_token=not-a-jwt"))
            .unwrap(),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(
        message(&reply.body),
        "Received backchannel logout request but OAuth is not enabled"
    );

    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/oauth/mobile-redirect?code=abc&state=xyz",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 307);
    assert_eq!(
        reply
            .headers
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("app.immich:///oauth-callback?code=abc&state=xyz")
    );
    let reply = call(
        &app,
        json_request("GET", "/api/oauth/mobile-redirect", None, None),
    )
    .await;
    assert_eq!(
        reply
            .headers
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("app.immich:///oauth-callback?")
    );

    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/link",
            None,
            Some(r#"{"url":"https://photos.example/cb"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 401);
    assert_eq!(message(&reply.body), "Authentication required");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/unlink",
            Some("immich_shared_link_token=link"),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 403);
    assert_eq!(message(&reply.body), "Forbidden");

    save_config(&app, &admin, |document| {
        document["oauth"]["enabled"] = json!(true);
        document["oauth"]["issuerUrl"] = json!(origin);
        document["oauth"]["clientId"] = json!("immich-client");
        document["oauth"]["clientSecret"] = json!(secret);
        document["oauth"]["signingAlgorithm"] = json!("HS256");
        document["oauth"]["allowInsecureRequests"] = json!(true);
        document["oauth"]["autoRegister"] = json!(true);
        document["oauth"]["endSessionEndpoint"] = json!("https://idp.example/session/end");
        document["oauth"]["mobileOverrideEnabled"] = json!(true);
        document["oauth"]["mobileRedirectUri"] = json!("https://photos.example/oauth/mobile");
    })
    .await;

    let reply = call(
        &app,
        json_request("GET", "/api/server/features", None, None),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(json_body(&reply.body)["oauth"], true);

    let again = sign_in(&app, "admin@example.com", "correct horse").await;
    assert!(again.starts_with("immich_access_token="));

    let reply = call(
        &app,
        json_request("POST", "/api/oauth/authorize", None, Some("{}")),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Validation failed");

    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/authorize",
            None,
            Some(r#"{"redirectUri":"app.immich:///oauth-callback"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 201, "{}", message(&reply.body));
    let started = json_body(&reply.body);
    let url = started["url"].as_str().unwrap();
    assert!(
        url.contains("redirect_uri=https%3A%2F%2Fphotos.example%2Foauth%2Fmobile"),
        "{url}"
    );
    assert!(url.contains("client_id=immich-client"), "{url}");
    assert!(url.contains("code_challenge_method=S256"), "{url}");
    let state = cookie_pair(&reply.headers, "immich_oauth_state").unwrap();
    let verifier = cookie_pair(&reply.headers, "immich_oauth_code_verifier").unwrap();
    assert!(cookie_header(&reply.headers, "immich_oauth_state")
        .unwrap()
        .contains("HttpOnly"));
    assert!(cookie_header(&reply.headers, "immich_oauth_code_verifier")
        .unwrap()
        .contains("HttpOnly"));
    let verifier_value = verifier.split_once('=').unwrap().1;
    let challenge = b64url(&Sha256::digest(verifier_value.as_bytes()));
    assert!(
        url.contains(&format!("code_challenge={challenge}")),
        "{url}"
    );
    let state_value = state.split_once('=').unwrap().1.to_string();

    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/callback",
            Some(&state),
            Some(&format!(
                r#"{{"url":"https://photos.example/auth/login?code=good&state={state_value}"}}"#
            )),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "OAuth code verifier is missing");

    let cookies = format!("{state}; {verifier}");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/callback",
            Some(&cookies),
            Some(&format!(
                r#"{{"url":"https://photos.example/auth/login?code=good&state={state_value}"}}"#
            )),
        ),
    )
    .await;
    let logged_in = json_body(&reply.body);
    assert_eq!(reply.status, 201, "{logged_in}");
    assert_eq!(logged_in["userEmail"], "admin@example.com");
    assert!(logged_in["isOnboarded"].is_boolean());
    assert!(cookie_header(&reply.headers, "immich_auth_type")
        .unwrap()
        .contains("immich_auth_type=oauth"));
    assert!(cookie_header(&reply.headers, "immich_auth_type")
        .unwrap()
        .contains("HttpOnly"));
    let authenticated = cookie_header(&reply.headers, "immich_is_authenticated").unwrap();
    assert!(!authenticated.contains("HttpOnly"));
    assert!(cookie_header(&reply.headers, "immich_oauth_state")
        .unwrap()
        .contains("Max-Age=0"));
    let access = cookie_pair(&reply.headers, "immich_access_token").unwrap();
    let me = call(
        &app,
        json_request("GET", "/api/users/me", Some(&access), None),
    )
    .await;
    assert_eq!(me.status, 200, "{}", message(&me.body));
    assert_eq!(json_body(&me.body)["oauthId"], "admin-sub");
    let groups_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cluster_group")
        .fetch_one(app.pool())
        .await
        .unwrap();

    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/authorize",
            None,
            Some(r#"{"redirectUri":"https://photos.example/auth/login"}"#),
        ),
    )
    .await;
    let state = cookie_pair(&reply.headers, "immich_oauth_state").unwrap();
    let verifier = cookie_pair(&reply.headers, "immich_oauth_code_verifier").unwrap();
    let state_value = state.split_once('=').unwrap().1;
    let cookies = format!("{state}; {verifier}");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/callback",
            Some(&cookies),
            Some(&format!(
                r#"{{"url":"https://photos.example/auth/login?code=new&state={state_value}"}}"#
            )),
        ),
    )
    .await;
    let created = json_body(&reply.body);
    assert_eq!(reply.status, 201, "{created}");
    assert_eq!(created["userEmail"], "new-person@example.com");
    assert_eq!(created["name"], "New Person");
    assert_eq!(created["isAdmin"], true);
    assert_eq!(created["shouldChangePassword"], true);
    assert_eq!(created["isOnboarded"], false);
    let new_access = cookie_pair(&reply.headers, "immich_access_token").unwrap();
    let me = call(
        &app,
        json_request("GET", "/api/users/me", Some(&new_access), None),
    )
    .await;
    let person = json_body(&me.body);
    assert_eq!(person["oauthId"], "new-sub");
    assert_eq!(person["storageLabel"], "newperson");
    assert_eq!(person["quotaSizeInBytes"], 1_073_741_824_i64);
    let groups_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cluster_group")
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(groups_after, groups_before);
    let password: Option<String> =
        sqlx::query_scalar("SELECT password FROM user WHERE email = 'new-person@example.com'")
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert!(password.is_none());

    save_config(&app, &admin, |document| {
        document["oauth"]["autoRegister"] = json!(false);
    })
    .await;
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/authorize",
            None,
            Some(r#"{"redirectUri":"https://photos.example/auth/login"}"#),
        ),
    )
    .await;
    let state = cookie_pair(&reply.headers, "immich_oauth_state").unwrap();
    let verifier = cookie_pair(&reply.headers, "immich_oauth_code_verifier").unwrap();
    let state_value = state.split_once('=').unwrap().1;
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/callback",
            Some(&format!("{state}; {verifier}")),
            Some(&format!(
                r#"{{"url":"https://photos.example/auth/login?code=unknown&state={state_value}"}}"#
            )),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "OAuth authentication failed");

    save_config(&app, &admin, |document| {
        document["oauth"]["autoRegister"] = json!(true);
    })
    .await;
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/authorize",
            None,
            Some(r#"{"redirectUri":"https://photos.example/auth/login"}"#),
        ),
    )
    .await;
    let state = cookie_pair(&reply.headers, "immich_oauth_state").unwrap();
    let verifier = cookie_pair(&reply.headers, "immich_oauth_code_verifier").unwrap();
    let state_value = state.split_once('=').unwrap().1;
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/callback",
            Some(&format!("{state}; {verifier}")),
            Some(&format!(
                r#"{{"url":"https://photos.example/auth/login?code=no-email&state={state_value}"}}"#
            )),
        ),
    )
    .await;
    assert_eq!(reply.status, 400, "{}", message(&reply.body));
    assert_eq!(
        message(&reply.body),
        "OAuth profile does not have an email address"
    );

    let guest_body = r#"{"email":"cam@example.com","password":"temp-pass","name":"Cam"}"#;
    let reply = call(
        &app,
        json_request("POST", "/api/admin/users", Some(&admin), Some(guest_body)),
    )
    .await;
    assert_eq!(reply.status, 201, "{}", message(&reply.body));
    sqlx::query("UPDATE user SET oauthId = 'taken-sub' WHERE email = 'cam@example.com'")
        .execute(app.pool())
        .await
        .unwrap();
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/authorize",
            None,
            Some(r#"{"redirectUri":"https://photos.example/auth/login"}"#),
        ),
    )
    .await;
    let state = cookie_pair(&reply.headers, "immich_oauth_state").unwrap();
    let verifier = cookie_pair(&reply.headers, "immich_oauth_code_verifier").unwrap();
    let state_value = state.split_once('=').unwrap().1;
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/callback",
            Some(&format!("{state}; {verifier}")),
            Some(&format!(
                r#"{{"url":"https://photos.example/auth/login?code=conflict&state={state_value}"}}"#
            )),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "OAuth authentication failed");

    let bea_body = r#"{"email":"bea@example.com","password":"temp-pass","name":"Bea"}"#;
    let reply = call(
        &app,
        json_request("POST", "/api/admin/users", Some(&admin), Some(bea_body)),
    )
    .await;
    assert_eq!(reply.status, 201, "{}", message(&reply.body));
    let bea = sign_in(&app, "bea@example.com", "temp-pass").await;
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/authorize",
            None,
            Some(r#"{"redirectUri":"https://photos.example/auth/login"}"#),
        ),
    )
    .await;
    let state = cookie_pair(&reply.headers, "immich_oauth_state").unwrap();
    let verifier = cookie_pair(&reply.headers, "immich_oauth_code_verifier").unwrap();
    let state_value = state.split_once('=').unwrap().1;
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/link",
            Some(&format!("{bea}; {state}; {verifier}")),
            Some(&format!(r#"{{"url":"https://photos.example/auth/login?code=link-admin&state={state_value}"}}"#)),
        ),
    )
    .await;
    assert_eq!(reply.status, 400, "{}", message(&reply.body));
    assert_eq!(
        message(&reply.body),
        "This OAuth account has already been linked to another user."
    );

    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/authorize",
            None,
            Some(r#"{"redirectUri":"https://photos.example/auth/login"}"#),
        ),
    )
    .await;
    let state = cookie_pair(&reply.headers, "immich_oauth_state").unwrap();
    let verifier = cookie_pair(&reply.headers, "immich_oauth_code_verifier").unwrap();
    let state_value = state.split_once('=').unwrap().1;
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/link",
            Some(&format!("{bea}; {state}; {verifier}")),
            Some(&format!(r#"{{"url":"https://photos.example/auth/login?code=link-guest&state={state_value}"}}"#)),
        ),
    )
    .await;
    let linked = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{linked}");
    assert_eq!(linked["oauthId"], "guest-oauth");
    let reply = call(
        &app,
        json_request("POST", "/api/oauth/unlink", Some(&bea), None),
    )
    .await;
    let unlinked = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{unlinked}");
    assert_eq!(unlinked["oauthId"], "");

    let stored: String = sqlx::query_scalar(
        "SELECT oauthBearerToken FROM session WHERE oauthSid = 'sid-admin' ORDER BY createdAt DESC",
    )
    .fetch_one(app.pool())
    .await
    .unwrap();
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/auth/logout",
            Some(&format!("{access}; immich_auth_type=oauth")),
            None,
        ),
    )
    .await;
    let logged_out = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{logged_out}");
    assert_eq!(
        logged_out["redirectUri"],
        format!("https://idp.example/session/end?id_token_hint={stored}")
    );
    assert_eq!(logged_out["successful"], true);

    let bad = sign_hs256(
        secret,
        &json!({
            "iss": origin,
            "aud": "immich-client",
            "iat": unix_now(),
            "exp": unix_now() + 60,
            "events": { "http://schemas.openid.net/event/backchannel-logout": {} }
        }),
    );
    let reply = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/oauth/backchannel-logout")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("logout_token={bad}")))
            .unwrap(),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(
        message(&reply.body),
        "Invalid logout token: it must contain either a sub or a sid claim"
    );

    let reply = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/oauth/backchannel-logout")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("logout_token=not-a-jwt"))
            .unwrap(),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(
        message(&reply.body),
        "Error backchannel logout: token validation failed"
    );

    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/authorize",
            None,
            Some(r#"{"redirectUri":"https://photos.example/auth/login"}"#),
        ),
    )
    .await;
    let state = cookie_pair(&reply.headers, "immich_oauth_state").unwrap();
    let verifier = cookie_pair(&reply.headers, "immich_oauth_code_verifier").unwrap();
    let state_value = state.split_once('=').unwrap().1;
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/oauth/callback",
            Some(&format!("{state}; {verifier}")),
            Some(&format!(
                r#"{{"url":"https://photos.example/auth/login?code=good&state={state_value}"}}"#
            )),
        ),
    )
    .await;
    assert_eq!(reply.status, 201, "{}", message(&reply.body));
    let fresh = cookie_pair(&reply.headers, "immich_access_token").unwrap();
    let living: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM session WHERE oauthSid = 'sid-admin'")
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(living, 1);

    let logout_token = sign_hs256(
        secret,
        &json!({
            "iss": origin,
            "aud": "immich-client",
            "sub": "admin-sub",
            "sid": "sid-admin",
            "iat": unix_now(),
            "exp": unix_now() + 60,
            "events": { "http://schemas.openid.net/event/backchannel-logout": {} }
        }),
    );
    let reply = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/oauth/backchannel-logout")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("logout_token={logout_token}")))
            .unwrap(),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", message(&reply.body));
    assert!(reply.body.is_empty());
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session WHERE oauthSid = 'sid-admin'")
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(left, 0);
    let me = call(
        &app,
        json_request("GET", "/api/users/me", Some(&fresh), None),
    )
    .await;
    assert_eq!(me.status, 401);

    let _ = std::fs::remove_dir_all(&dir);
}
