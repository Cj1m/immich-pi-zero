//! A file still on the Pi is served, then copied into the local store when the spool is resumed.

use std::net::IpAddr;
use std::path::Path;

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
        machine_learning_enabled: false,
        config_file: None,
        log_level: None,
    }
}

async fn call(app: &App, request: Request<Body>) -> (http::StatusCode, Vec<u8>) {
    let response = immich_server::router(app.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

fn jpeg() -> Vec<u8> {
    let mut image = RgbImage::new(8, 8);
    for pixel in image.pixels_mut() {
        *pixel = Rgb([9, 8, 7]);
    }
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn upload(cookie: &str, bytes: &[u8]) -> Request<Body> {
    let boundary = "immichboundary";
    let mut body = Vec::new();
    for (name, value) in [
        ("deviceAssetId", "spool-1"),
        ("deviceId", "phone"),
        ("fileCreatedAt", "2020-01-02T03:04:05.000Z"),
        ("fileModifiedAt", "2020-01-02T03:04:05.000Z"),
        ("filename", "photo.jpg"),
    ] {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").into_bytes(),
        );
    }
    body.extend(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"photo.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n").into_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    Request::builder()
        .method("POST")
        .uri("/api/assets")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("cookie", cookie)
        .body(Body::from(body))
        .unwrap()
}

async fn sign_in(app: &App) -> String {
    let response = immich_server::router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correct horse"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("immich_access_token="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn spooled_original_is_served_and_then_stored() {
    let dir = std::env::temp_dir().join(format!("immich-spool-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let cookie = sign_in(&app).await;
    let photo = jpeg();
    let (status, body) = call(&app, upload(&cookie, &photo)).await;
    let created = json_body(&body);
    assert_eq!(status, 201, "{created}");
    let asset_id = created["id"].as_str().unwrap();

    let (status, me) = call(
        &app,
        Request::builder()
            .uri("/api/users/me")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    let owner = json_body(&me)["id"].as_str().unwrap().to_string();
    let stored = dir
        .join("objects")
        .join(&owner)
        .join(asset_id)
        .join("original");
    let spool = dir.join("spool").join(asset_id);
    std::fs::create_dir_all(&spool).unwrap();
    std::fs::rename(&stored, spool.join("original")).unwrap();

    let (status, bytes) = call(
        &app,
        Request::builder()
            .uri(format!("/api/assets/{asset_id}/original"))
            .header("cookie", &cookie)
            .header("range", "bytes=0-2")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 206);
    assert_eq!(bytes, photo[..3]);

    let orphan = dir.join("spool").join("not-an-asset");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("original"), b"gone").unwrap();

    app.resume_spooled_uploads().await;

    assert!(!spool.join("original").exists());
    assert!(!orphan.exists());
    assert_eq!(std::fs::read(&stored).unwrap(), photo);
    let (status, bytes) = call(
        &app,
        Request::builder()
            .uri(format!("/api/assets/{asset_id}/original"))
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(bytes, photo);
    let _ = std::fs::remove_dir_all(&dir);
}
