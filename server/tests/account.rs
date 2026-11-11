use std::net::IpAddr;
use std::path::Path;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
use image::{ImageFormat, Rgb, RgbImage};
use immich_server::{App, Config};
use serde_json::Value;
use tower::ServiceExt;

fn config(dir: &Path) -> Config {
    Config {
        data_dir: dir.to_path_buf(),
        host: IpAddr::from([127, 0, 0, 1]),
        port: 0,
        admin_email: Some("admin@example.com".to_string()),
        admin_name: Some("Ada".to_string()),
        admin_password: Some("correct horse".to_string()),
        s3: None,
        web_root: dir.join("web"),
        machine_learning_url: "http://immich-machine-learning:3003".to_string(),
        machine_learning_enabled: true,
        config_file: None,
        log_level: None,
    }
}

async fn call(app: &App, request: Request<Body>) -> (http::StatusCode, Vec<u8>) {
    let response = immich_server::router(app.clone())
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, bytes.to_vec())
}

fn json_request(
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    builder
        .body(Body::from(body.unwrap_or("").to_string()))
        .expect("request")
}

fn api_request(method: &str, path: &str, api_key: &str, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("x-api-key", api_key);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(Body::from(body.unwrap_or("").to_string()))
        .expect("request")
}

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

async fn login(app: &App, email: &str, password: &str) -> (String, String) {
    let (status, body) = call(
        app,
        Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .header(
                "user-agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/120.0.0.0 Safari/537.36",
            )
            .header("devicemodel", "Mac")
            .header("devicetype", "desktop")
            .body(Body::from(format!(
                r#"{{"email":"{email}","password":"{password}"}}"#
            )))
            .unwrap(),
    )
    .await;
    let body = json_body(&body);
    assert_eq!(status, 201, "{body}");
    let user_id = body["userId"].as_str().unwrap().to_string();
    let token = body["accessToken"].as_str().unwrap().to_string();
    (user_id, format!("immich_access_token={token}"))
}

fn jpeg(red: u8) -> Vec<u8> {
    let image = RgbImage::from_pixel(32, 24, Rgb([red, 80, 160]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn profile_upload(cookie: &str, bytes: &[u8], filename: &str) -> Request<Body> {
    let boundary = "profileboundary";
    let mut body = Vec::new();
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .into_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    Request::builder()
        .method("POST")
        .uri("/api/users/profile-image")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("cookie", cookie)
        .body(Body::from(body))
        .unwrap()
}

async fn sync_lines(app: &App, cookie: &str, body: &str) -> Vec<Value> {
    let (status, bytes) = call(
        app,
        json_request("POST", "/api/sync/stream", Some(cookie), Some(body)),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_socket(addr: std::net::SocketAddr, cookie: &str) -> Socket {
    let mut request = format!("ws://{addr}/api/socket.io/?EIO=4&transport=websocket")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("socket");
    let opened = next_text(&mut socket).await;
    assert!(opened.starts_with('0'), "{opened}");
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text("40".into()))
        .await
        .unwrap();
    let connected = next_text(&mut socket).await;
    assert!(connected.starts_with("40{"), "{connected}");
    assert!(connected.contains("\"sid\""), "{connected}");
    let version = next_text(&mut socket).await;
    assert!(version.contains("on_server_version"), "{version}");
    assert!(version.contains("\"major\":3"), "{version}");
    assert!(version.contains("\"patch\":6"), "{version}");
    socket
}

async fn next_text(socket: &mut Socket) -> String {
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .expect("socket timed out")
        .expect("socket closed")
        .expect("socket error");
    message.to_text().expect("text").to_string()
}

#[tokio::test]
async fn signed_in_account_settings() {
    let dir = std::env::temp_dir().join(format!("immich-account-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let (admin_id, admin_cookie) = login(&app, "admin@example.com", "correct horse").await;

    let (status, body) = call(&app, json_request("GET", "/api/api-keys", None, None)).await;
    assert_eq!(status, 401, "{}", String::from_utf8_lossy(&body));
    assert_eq!(json_body(&body)["message"], "Authentication required");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/albums",
            Some(&admin_cookie),
            Some(r#"{"albumName":"Trip"}"#),
        ),
    )
    .await;
    let album = json_body(&body);
    assert_eq!(status, 201, "{album}");
    let album_id = album["id"].as_str().unwrap();
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/shared-links",
            Some(&admin_cookie),
            Some(&format!(
                r#"{{"type":"ALBUM","albumId":"{album_id}","slug":"acct"}}"#
            )),
        ),
    )
    .await;
    let link = json_body(&body);
    assert_eq!(status, 201, "{link}");
    let share_key = link["key"].as_str().unwrap();
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/api-keys?key={share_key}"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 403, "{}", json_body(&body));
    assert_eq!(json_body(&body)["message"], "Forbidden");

    let (status, body) = call(&app, api_request("GET", "/api/api-keys", "not-a-key", None)).await;
    assert_eq!(status, 401, "{}", json_body(&body));
    assert_eq!(json_body(&body)["message"], "Invalid API key");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/api-keys",
            Some(&admin_cookie),
            Some(r#"{"permissions":[]}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(json_body(&body)["message"], "Validation failed");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/api-keys",
            Some(&admin_cookie),
            Some(r#"{"permissions":["all"]}"#),
        ),
    )
    .await;
    let created = json_body(&body);
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["name"], "API Key");
    assert_eq!(created["permissions"][0], "all");
    assert!(created["secret"].as_str().unwrap().len() > 20);
    assert_eq!(created["apiKey"]["id"], created["id"]);
    assert!(created.get("key").is_none());
    let key_id = created["id"].as_str().unwrap().to_string();

    let (status, body) = call(
        &app,
        json_request("GET", "/api/api-keys", Some(&admin_cookie), None),
    )
    .await;
    let listed = json_body(&body);
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["name"], "API Key");
    assert!(listed[0].get("secret").is_none());
    assert!(listed[0].get("key").is_none());

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/api-keys/{key_id}"),
            Some(&admin_cookie),
            Some(r#"{"name":"Phone","permissions":["asset.read","apiKey.read"]}"#),
        ),
    )
    .await;
    let updated = json_body(&body);
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["name"], "Phone");
    assert_eq!(updated["permissions"][0], "asset.read");

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/api-keys/00000000-0000-4000-8000-000000000000",
            Some(&admin_cookie),
            Some(r#"{"name":"Missing"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(json_body(&body)["message"], "API Key not found");

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/api-keys/not-a-uuid",
            Some(&admin_cookie),
            Some(r#"{"name":"Bad"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json_body(&body)["message"], "Validation failed");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/api-keys",
            Some(&admin_cookie),
            Some(
                r#"{"name":"Limited","permissions":["apiKey.create","apiKey.read","apiKey.rotate","asset.read","session.delete"]}"#,
            ),
        ),
    )
    .await;
    let limited = json_body(&body);
    assert_eq!(status, 201, "{limited}");
    let limited_secret = limited["secret"].as_str().unwrap().to_string();

    let (status, body) = call(
        &app,
        api_request(
            "POST",
            "/api/api-keys",
            &limited_secret,
            Some(r#"{"permissions":["asset.update"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(
        json_body(&body)["message"],
        "Cannot grant permissions you do not have"
    );

    let (status, body) = call(
        &app,
        api_request(
            "POST",
            "/api/api-keys",
            &limited_secret,
            Some(r#"{"name":"Child","permissions":["asset.read"]}"#),
        ),
    )
    .await;
    let child = json_body(&body);
    assert_eq!(status, 201, "{child}");
    let child_id = child["id"].as_str().unwrap().to_string();
    assert_eq!(child["name"], "Child");

    let (status, body) = call(
        &app,
        api_request("GET", "/api/sessions", &limited_secret, None),
    )
    .await;
    assert_eq!(status, 403, "{}", json_body(&body));
    assert_eq!(
        json_body(&body)["message"],
        "Missing required permission: session.read"
    );

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/api-keys",
            Some(&admin_cookie),
            Some(r#"{"name":"Broad","permissions":["all"]}"#),
        ),
    )
    .await;
    let broad = json_body(&body);
    assert_eq!(status, 201, "{broad}");
    let broad_id = broad["id"].as_str().unwrap();
    let (status, body) = call(
        &app,
        api_request(
            "POST",
            &format!("/api/api-keys/{broad_id}/rotate"),
            &limited_secret,
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(
        json_body(&body)["message"],
        "Cannot rotate an API Key with permissions you do not have"
    );

    let (status, body) = call(
        &app,
        api_request(
            "POST",
            &format!("/api/api-keys/{child_id}/rotate"),
            &limited_secret,
            None,
        ),
    )
    .await;
    let rotated = json_body(&body);
    assert_eq!(status, 201, "{rotated}");
    let rotated_secret = rotated["secret"].as_str().unwrap().to_string();
    assert_ne!(rotated_secret, child["secret"].as_str().unwrap());
    let (status, _) = call(
        &app,
        api_request("GET", "/api/api-keys", &rotated_secret, None),
    )
    .await;
    assert_eq!(status, 403);
    let (status, body) = call(
        &app,
        api_request(
            "GET",
            "/api/api-keys",
            child["secret"].as_str().unwrap(),
            None,
        ),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(json_body(&body)["message"], "Invalid API key");

    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/api-keys/{child_id}"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));
    assert!(body.is_empty());
    let (status, body) = call(
        &app,
        api_request("GET", "/api/api-keys", &rotated_secret, None),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(json_body(&body)["message"], "Invalid API key");

    let (status, body) = call(
        &app,
        json_request("GET", "/api/sessions", Some(&admin_cookie), None),
    )
    .await;
    let sessions = json_body(&body);
    assert_eq!(status, 200, "{sessions}");
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["current"], true);
    assert_eq!(sessions[0]["deviceType"], "Chrome");
    assert_eq!(sessions[0]["deviceOS"], "Mac OS");
    assert!(sessions[0]["appVersion"].is_null());
    assert!(sessions[0].get("expiresAt").is_none());
    let first_session = sessions[0]["id"].as_str().unwrap().to_string();

    let (_, second_cookie) = login(&app, "admin@example.com", "correct horse").await;
    let (status, body) = call(
        &app,
        json_request("GET", "/api/sessions", Some(&second_cookie), None),
    )
    .await;
    let sessions = json_body(&body);
    assert_eq!(status, 200, "{sessions}");
    assert_eq!(sessions.as_array().unwrap().len(), 2);
    let current = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["current"] == true)
        .unwrap();
    assert_ne!(current["id"], first_session);
    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/sessions/{first_session}"),
            Some(&second_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));
    let (status, body) = call(
        &app,
        json_request("GET", "/api/api-keys", Some(&admin_cookie), None),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(json_body(&body)["message"], "Invalid user token");

    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            "/api/sessions/00000000-0000-4000-8000-000000000000",
            Some(&second_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        json_body(&body)["message"],
        "Not found or no authDevice.delete access"
    );

    let (_, third_cookie) = login(&app, "admin@example.com", "correct horse").await;
    let (status, _) = call(
        &app,
        json_request("DELETE", "/api/sessions", Some(&second_cookie), None),
    )
    .await;
    assert_eq!(status, 204);
    let (status, _) = call(
        &app,
        json_request("GET", "/api/sessions", Some(&second_cookie), None),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = call(
        &app,
        json_request("GET", "/api/sessions", Some(&third_cookie), None),
    )
    .await;
    assert_eq!(status, 401);

    let (status, _) = call(
        &app,
        api_request("DELETE", "/api/sessions", &limited_secret, None),
    )
    .await;
    assert_eq!(status, 204);
    let (status, body) = call(
        &app,
        json_request("GET", "/api/sessions", Some(&second_cookie), None),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(json_body(&body)["message"], "Invalid user token");

    let (_, admin_cookie) = login(&app, "admin@example.com", "correct horse").await;

    let photo = jpeg(32);
    let (status, body) = call(&app, profile_upload(&admin_cookie, &photo, "me.jpg")).await;
    let profile = json_body(&body);
    assert_eq!(status, 201, "{profile}");
    assert_eq!(profile["userId"], admin_id);
    let profile_path = profile["profileImagePath"].as_str().unwrap().to_string();
    assert!(profile_path.contains("/profile/"));
    assert!(std::path::Path::new(&profile_path).is_file());
    assert!(profile["profileChangedAt"].as_str().unwrap().contains('T'));
    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/users/{admin_id}/profile-image"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert!(bytes.starts_with(&[0xFF, 0xD8]));
    let lines = sync_lines(&app, &admin_cookie, r#"{"types":["UsersV1"]}"#).await;
    let synced = lines
        .iter()
        .find(|line| line["type"] == "UserV1" && line["data"]["id"] == admin_id)
        .unwrap();
    assert_eq!(synced["data"]["hasProfileImage"], true);

    let (status, body) = call(
        &app,
        profile_upload(&admin_cookie, b"not-a-picture", "bad.jpg"),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(
        json_body(&body)["message"],
        "Unable to process profile image"
    );
    assert!(std::path::Path::new(&profile_path).is_file());

    let (status, body) = call(&app, profile_upload(&admin_cookie, &jpeg(200), "next.jpg")).await;
    let replaced = json_body(&body);
    assert_eq!(status, 201, "{replaced}");
    let replaced_path = replaced["profileImagePath"].as_str().unwrap();
    assert_ne!(replaced_path, profile_path);
    assert!(!std::path::Path::new(&profile_path).exists());
    assert!(std::path::Path::new(replaced_path).is_file());

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(&admin_cookie),
            Some(r#"{"email":"guest@example.com","password":"guest-pass","name":"Guest"}"#),
        ),
    )
    .await;
    let guest = json_body(&body);
    assert_eq!(status, 201, "{guest}");
    let guest_id = guest["id"].as_str().unwrap().to_string();
    let (_, guest_cookie) = login(&app, "guest@example.com", "guest-pass").await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = immich_server::router(app.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, server).await;
    });
    let mut admin_socket = connect_socket(addr, &admin_cookie).await;
    let mut guest_socket = connect_socket(addr, &guest_cookie).await;
    let note = app
        .create_notification(
            &admin_id,
            "info",
            "Library ready",
            Some("Thumbnails finished"),
            Some(serde_json::json!({ "albumId": album_id })),
        )
        .await
        .expect("notification");
    let event = next_text(&mut admin_socket).await;
    assert!(event.contains("on_notification"), "{event}");
    assert!(event.contains("Library ready"), "{event}");
    let quiet =
        tokio::time::timeout(std::time::Duration::from_millis(400), guest_socket.next()).await;
    assert!(quiet.is_err(), "guest received {quiet:?}");

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/notifications?unread=true",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let unread = json_body(&body);
    assert_eq!(status, 200, "{unread}");
    assert_eq!(unread.as_array().unwrap().len(), 1);
    assert_eq!(unread[0]["id"], note["id"]);
    assert_eq!(unread[0]["title"], "Library ready");
    assert_eq!(unread[0]["description"], "Thumbnails finished");
    assert_eq!(unread[0]["data"]["albumId"], album_id);
    assert_eq!(unread[0]["level"], "info");
    assert_eq!(unread[0]["type"], "info");
    assert!(unread[0].get("readAt").is_none());

    let guest_note = app
        .create_notification(&guest_id, "info", "Guest only", None, None)
        .await
        .unwrap();
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/notifications",
            Some(&admin_cookie),
            Some(&format!(
                r#"{{"ids":["{}","{}"],"readAt":"2026-11-08T00:00:00.000Z"}}"#,
                note["id"].as_str().unwrap(),
                guest_note["id"].as_str().unwrap()
            )),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(
        json_body(&body)["message"],
        "Not found or no notification.update access"
    );
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/notifications/{}", note["id"].as_str().unwrap()),
            Some(&admin_cookie),
            Some(r#"{"readAt":"2026-11-08T01:02:03.000Z"}"#),
        ),
    )
    .await;
    let read = json_body(&body);
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["readAt"], "2026-11-08T01:02:03.000Z");
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/notifications?unread=true",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let unread = json_body(&body);
    assert_eq!(status, 200, "{unread}");
    assert!(unread.as_array().unwrap().is_empty());

    let extra = app
        .create_notification(&admin_id, "info", "Second", None, None)
        .await
        .unwrap();
    let (status, _) = call(
        &app,
        json_request(
            "PUT",
            "/api/notifications",
            Some(&admin_cookie),
            Some(&format!(
                r#"{{"ids":["{}"],"readAt":"2026-11-08T02:00:00.000Z"}}"#,
                extra["id"].as_str().unwrap()
            )),
        ),
    )
    .await;
    assert_eq!(status, 204);

    for sample in [
        SampleAsset {
            kind: "IMAGE",
            visibility: "timeline",
            favorite: false,
            trashed: false,
            mark: 1,
            created_at: "2020-01-02T03:00:00.000Z",
            local_at: "2020-01-01T03:00:00.000Z",
        },
        SampleAsset {
            kind: "VIDEO",
            visibility: "timeline",
            favorite: false,
            trashed: false,
            mark: 2,
            created_at: "2020-01-02T04:00:00.000Z",
            local_at: "2020-01-02T04:00:00.000Z",
        },
        SampleAsset {
            kind: "AUDIO",
            visibility: "timeline",
            favorite: false,
            trashed: false,
            mark: 3,
            created_at: "2020-01-02T05:00:00.000Z",
            local_at: "2020-01-02T05:00:00.000Z",
        },
        SampleAsset {
            kind: "OTHER",
            visibility: "archive",
            favorite: false,
            trashed: false,
            mark: 4,
            created_at: "2020-01-02T06:00:00.000Z",
            local_at: "2020-01-02T06:00:00.000Z",
        },
        SampleAsset {
            kind: "IMAGE",
            visibility: "archive",
            favorite: true,
            trashed: false,
            mark: 5,
            created_at: "2020-01-02T07:00:00.000Z",
            local_at: "2020-01-02T07:00:00.000Z",
        },
        SampleAsset {
            kind: "IMAGE",
            visibility: "hidden",
            favorite: false,
            trashed: false,
            mark: 6,
            created_at: "2020-01-02T08:00:00.000Z",
            local_at: "2020-01-02T08:00:00.000Z",
        },
        SampleAsset {
            kind: "IMAGE",
            visibility: "timeline",
            favorite: false,
            trashed: true,
            mark: 7,
            created_at: "2020-01-03T08:00:00.000Z",
            local_at: "2020-01-03T08:00:00.000Z",
        },
        SampleAsset {
            kind: "IMAGE",
            visibility: "locked",
            favorite: false,
            trashed: false,
            mark: 8,
            created_at: "2020-01-02T09:00:00.000Z",
            local_at: "2020-01-02T09:00:00.000Z",
        },
    ] {
        insert_asset(&app, &admin_id, sample).await;
    }

    let (status, body) = call(
        &app,
        json_request("GET", "/api/assets/statistics", Some(&admin_cookie), None),
    )
    .await;
    let stats = json_body(&body);
    assert_eq!(status, 200, "{stats}");
    assert_eq!(stats["images"], 2);
    assert_eq!(stats["videos"], 1);
    assert_eq!(stats["total"], 5);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/assets/statistics?visibility=timeline",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let stats = json_body(&body);
    assert_eq!(status, 200, "{stats}");
    assert_eq!(stats["images"], 1);
    assert_eq!(stats["videos"], 1);
    assert_eq!(stats["total"], 3);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/assets/statistics?isFavorite=true",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let stats = json_body(&body);
    assert_eq!(status, 200, "{stats}");
    assert_eq!(stats["images"], 1);
    assert_eq!(stats["total"], 1);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/assets/statistics?isTrashed=true",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let stats = json_body(&body);
    assert_eq!(status, 200, "{stats}");
    assert_eq!(stats["images"], 1);
    assert_eq!(stats["total"], 1);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/assets/statistics?visibility=locked",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 401, "{}", json_body(&body));
    assert_eq!(
        json_body(&body)["message"],
        "Elevated permission is required"
    );

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/assets/statistics?isFavorite=yes",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json_body(&body)["message"], "Validation failed");

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/users/me/calendar-heatmap?from=2020-01-01&to=2020-01-03",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let heatmap = json_body(&body);
    assert_eq!(status, 200, "{heatmap}");
    assert_eq!(heatmap["series"].as_array().unwrap().len(), 3);
    assert_eq!(heatmap["series"][1]["date"], "2020-01-02");
    assert_eq!(heatmap["series"][1]["count"], 7);
    assert_eq!(heatmap["totalCount"], 7);
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/users/me/calendar-heatmap?from=2020-01-01&to=2020-01-03&type=Taken",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let taken = json_body(&body);
    assert_eq!(status, 200, "{taken}");
    assert_eq!(taken["series"][0]["date"], "2020-01-01");
    assert_eq!(taken["series"][0]["count"], 1);
    assert_eq!(taken["totalCount"], 7);

    let _ = std::fs::remove_dir_all(&dir);
}

struct SampleAsset {
    kind: &'static str,
    visibility: &'static str,
    favorite: bool,
    trashed: bool,
    mark: u8,
    created_at: &'static str,
    local_at: &'static str,
}

async fn insert_asset(app: &App, owner_id: &str, sample: SampleAsset) {
    let deleted_at = if sample.trashed {
        Some("2020-02-01T00:00:00.000Z")
    } else {
        None
    };
    sqlx::query(
        "INSERT INTO asset (
            id, ownerId, type, originalPath, originalFileName, originalMimeType,
            fileCreatedAt, fileModifiedAt, localDateTime, checksum, visibility,
            isFavorite, deletedAt, status, createdAt, updatedAt, updateId
        ) VALUES (?, ?, ?, 'object', 'a.jpg', 'image/jpeg', ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(owner_id)
    .bind(sample.kind)
    .bind(sample.created_at)
    .bind(sample.created_at)
    .bind(sample.local_at)
    .bind(vec![sample.mark, 9, 9])
    .bind(sample.visibility)
    .bind(i64::from(sample.favorite))
    .bind(deleted_at)
    .bind(sample.created_at)
    .bind(sample.created_at)
    .bind(uuid::Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .expect("asset");
}
