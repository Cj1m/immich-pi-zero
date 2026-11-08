//! Upload check, asset jobs, and the signed-in download zip.

use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
use base64::Engine;
use http::Request;
use http_body_util::BodyExt;
use image::{ImageFormat, Rgb, RgbImage};
use immich_server::{App, Config};
use serde_json::Value;
use sha1::{Digest, Sha1};
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

async fn call(app: &App, request: Request<Body>) -> (http::StatusCode, Vec<u8>, Option<String>) {
    let response = immich_server::router(app.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let disposition = response
        .headers()
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes, disposition)
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

fn jpeg(mark: u8) -> Vec<u8> {
    let mut image = RgbImage::new(16, 16);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = Rgb([mark, x as u8, y as u8]);
    }
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn upload(cookie: &str, bytes: &[u8], device_asset_id: &str, created_at: &str) -> Request<Body> {
    let boundary = "immichboundary";
    let mut body = Vec::new();
    for (name, value) in [
        ("deviceAssetId", device_asset_id),
        ("deviceId", "phone"),
        ("fileCreatedAt", created_at),
        ("fileModifiedAt", created_at),
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

async fn sign_in(app: &App, email: &str, password: &str) -> String {
    let response = immich_server::router(app.clone())
        .oneshot(json_request(
            "POST",
            "/api/auth/login",
            None,
            Some(&format!(r#"{{"email":"{email}","password":"{password}"}}"#)),
        ))
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

fn digest(bytes: &[u8]) -> Vec<u8> {
    Sha1::digest(bytes).to_vec()
}

#[tokio::test]
async fn upload_check_jobs_and_download() {
    let dir = std::env::temp_dir().join(format!("immich-jobs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let admin = sign_in(&app, "admin@example.com", "correct horse").await;
    let (status, body, _) = call(
        &app,
        json_request("GET", "/api/users/me", Some(&admin), None),
    )
    .await;
    let me = json_body(&body);
    assert_eq!(status, 200, "{me}");
    let admin_id = me["id"].as_str().unwrap().to_string();

    let photo = jpeg(4);
    let hex = hex::encode(digest(&photo));
    let b64 = base64::engine::general_purpose::STANDARD.encode(digest(&photo));
    assert_eq!(b64.len(), 28);
    let (status, body, _) = call(
        &app,
        upload(&admin, &photo, "device-a", "2020-01-02T03:04:05.000Z"),
    )
    .await;
    let created = json_body(&body);
    assert_eq!(status, 201, "{created}");
    let asset_id = created["id"].as_str().unwrap().to_string();

    let (status, body, _) = call(
        &app,
        json_request("POST", "/api/assets/bulk-upload-check", None, Some("{}")),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(message(&body), "Authentication required");
    let (status, body, _) = call(
        &app,
        json_request(
            "POST",
            "/api/assets/jobs",
            Some("immich_shared_link_token=abc"),
            Some(r#"{"name":"refresh-faces","assetIds":[]}"#),
        ),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(message(&body), "Forbidden");
    let (status, body, _) = call(
        &app,
        json_request(
            "POST",
            "/api/assets/bulk-upload-check?key=abc",
            Some(&admin),
            Some("{}"),
        ),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(message(&body), "Invalid share key");

    let check = format!(
        r#"{{"assets":[{{"id":"hex","checksum":"{hex}"}},{{"id":"b64","checksum":"{b64}"}},{{"id":"new","checksum":"abcd"}}]}}"#
    );
    let (status, body, _) = call(
        &app,
        json_request(
            "POST",
            "/api/assets/bulk-upload-check",
            Some(&admin),
            Some(&check),
        ),
    )
    .await;
    let results = json_body(&body);
    assert_eq!(status, 200, "{results}");
    assert_eq!(results["results"][0]["id"], "hex");
    assert_eq!(results["results"][0]["action"], "reject");
    assert_eq!(results["results"][0]["reason"], "duplicate");
    assert_eq!(results["results"][0]["assetId"], asset_id);
    assert_eq!(results["results"][0]["isTrashed"], false);
    assert_eq!(results["results"][1]["id"], "b64");
    assert_eq!(results["results"][1]["action"], "reject");
    assert_eq!(results["results"][1]["assetId"], asset_id);
    assert_eq!(results["results"][2]["id"], "new");
    assert_eq!(results["results"][2]["action"], "accept");

    let (status, created_user, _) = call(
        &app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(&admin),
            Some(r#"{"email":"guest@example.com","password":"temp-pass","name":"Guest","notify":false}"#),
        ),
    )
    .await;
    let guest = json_body(&created_user);
    assert_eq!(status, 201, "{guest}");
    let guest_cookie = sign_in(&app, "guest@example.com", "temp-pass").await;
    let (status, body, _) = call(
        &app,
        json_request(
            "POST",
            "/api/assets/bulk-upload-check",
            Some(&guest_cookie),
            Some(&format!(
                r#"{{"assets":[{{"id":"x","checksum":"{hex}"}}]}}"#
            )),
        ),
    )
    .await;
    let other = json_body(&body);
    assert_eq!(status, 200, "{other}");
    assert_eq!(other["results"][0]["action"], "accept");

    let (status, body, _) = call(
        &app,
        json_request(
            "DELETE",
            "/api/assets",
            Some(&admin),
            Some(&format!(r#"{{"ids":["{asset_id}"]}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));
    let (status, body, _) = call(
        &app,
        json_request(
            "POST",
            "/api/assets/bulk-upload-check",
            Some(&admin),
            Some(&format!(
                r#"{{"assets":[{{"id":"hex","checksum":"{hex}"}}]}}"#
            )),
        ),
    )
    .await;
    let trashed = json_body(&body);
    assert_eq!(status, 200, "{trashed}");
    assert_eq!(trashed["results"][0]["isTrashed"], true);
    assert_eq!(trashed["results"][0]["assetId"], asset_id);

    let (status, _, _) = call(
        &app,
        json_request(
            "POST",
            "/api/trash/restore/assets",
            Some(&admin),
            Some(&format!(r#"{{"ids":["{asset_id}"]}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 200);

    let archive_photo = jpeg(9);
    let (status, body, _) = call(
        &app,
        upload(
            &admin,
            &archive_photo,
            "device-b",
            "2020-02-02T03:04:05.000Z",
        ),
    )
    .await;
    let archive_created = json_body(&body);
    assert_eq!(status, 201, "{archive_created}");
    let archive_id = archive_created["id"].as_str().unwrap().to_string();
    let hidden_photo = jpeg(12);
    let (status, body, _) = call(
        &app,
        upload(
            &admin,
            &hidden_photo,
            "device-c",
            "2020-03-02T03:04:05.000Z",
        ),
    )
    .await;
    let hidden_created = json_body(&body);
    assert_eq!(status, 201, "{hidden_created}");
    let hidden_id = hidden_created["id"].as_str().unwrap().to_string();
    let (status, body, _) = call(
        &app,
        json_request(
            "PUT",
            "/api/assets",
            Some(&admin),
            Some(&format!(
                r#"{{"ids":["{archive_id}"],"visibility":"archive"}}"#
            )),
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));
    let (status, body, _) = call(
        &app,
        json_request(
            "PUT",
            "/api/assets",
            Some(&admin),
            Some(&format!(
                r#"{{"ids":["{hidden_id}"],"visibility":"hidden"}}"#
            )),
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));

    let faces_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset_face")
        .fetch_one(app.pool())
        .await
        .unwrap();
    let files_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset_file")
        .fetch_one(app.pool())
        .await
        .unwrap();
    let (status, body, _) = call(
        &app,
        json_request(
            "POST",
            "/api/assets/jobs",
            Some(&admin),
            Some(r#"{"name":"not-a-job","assetIds":[]}"#),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&body), "Validation failed");
    let missing = "99999999-9999-4999-8999-999999999999";
    let (status, body, _) = call(
        &app,
        json_request(
            "POST",
            "/api/assets/jobs",
            Some(&admin),
            Some(&format!(
                r#"{{"name":"refresh-metadata","assetIds":["{missing}"]}}"#
            )),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(message(&body), "Not found or no asset.update access");
    let (status, body, _) = call(
        &app,
        json_request(
            "POST",
            "/api/assets/jobs",
            Some(&guest_cookie),
            Some(&format!(
                r#"{{"name":"regenerate-thumbnail","assetIds":["{asset_id}"]}}"#
            )),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&body), "Not found or no asset.update access");
    for name in [
        "refresh-faces",
        "refresh-metadata",
        "regenerate-thumbnail",
        "transcode-video",
    ] {
        let (status, body, _) = call(
            &app,
            json_request(
                "POST",
                "/api/assets/jobs",
                Some(&admin),
                Some(&format!(r#"{{"name":"{name}","assetIds":["{asset_id}"]}}"#)),
            ),
        )
        .await;
        assert_eq!(status, 204, "{name} {}", String::from_utf8_lossy(&body));
    }
    let faces_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset_face")
        .fetch_one(app.pool())
        .await
        .unwrap();
    let files_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset_file")
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(faces_after, faces_before);
    assert_eq!(files_after, files_before);

    let (status, body, _) = call(
        &app,
        json_request("POST", "/api/download/info", Some(&admin), Some("{}")),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(message(&body), "assetIds, albumId, or userId is required");
    let (status, body, _) = call(
        &app,
        json_request(
            "POST",
            "/api/download/info",
            Some(&admin),
            Some(&format!(r#"{{"userId":"{admin_id}","archiveSize":1}}"#)),
        ),
    )
    .await;
    let info = json_body(&body);
    assert_eq!(status, 201, "{info}");
    let archives = info["archives"].as_array().unwrap();
    assert!(archives.len() >= 2, "{info}");
    let ids: Vec<String> = archives
        .iter()
        .flat_map(|archive| archive["assetIds"].as_array().unwrap().clone())
        .map(|id| id.as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&asset_id), "{info}");
    assert!(ids.contains(&archive_id), "{info}");
    assert!(!ids.contains(&hidden_id), "{info}");
    assert_eq!(
        info["totalSize"].as_i64().unwrap(),
        archives
            .iter()
            .map(|archive| archive["size"].as_i64().unwrap())
            .sum::<i64>()
    );

    let (status, zip, disposition) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/download/archive")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", &admin)
            .body(Body::from(format!("assetIds={asset_id},{missing}")))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&zip));
    assert!(disposition.is_none());
    assert_eq!(&zip[..2], b"PK");
    assert!(zip.windows(photo.len()).any(|window| window == photo));

    let edited_rel = format!("edited/{asset_id}.bin");
    let edited_path = dir.join(&edited_rel);
    std::fs::create_dir_all(edited_path.parent().unwrap()).unwrap();
    let marker = b"EDITED-FULLSIZE-MARKER";
    std::fs::write(&edited_path, marker).unwrap();
    sqlx::query("INSERT INTO asset_file (id, assetId, type, path) VALUES (?, ?, 'fullsize', ?)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&asset_id)
        .bind(&edited_rel)
        .execute(app.pool())
        .await
        .unwrap();
    let (status, edited_zip, disposition) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/download/archive")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", &admin)
            .body(Body::from(format!(
                "assetIds={asset_id}&archiveName=Summer&edited=true"
            )))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&edited_zip));
    assert_eq!(
        disposition.as_deref(),
        Some("attachment; filename*=UTF-8''Summer.zip")
    );
    assert!(edited_zip
        .windows(marker.len())
        .any(|window| window == marker));
    assert!(!edited_zip
        .windows(photo.len())
        .any(|window| window == photo));

    let (status, body, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/download/archive")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", &guest_cookie)
            .body(Body::from(format!("assetIds={asset_id}")))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(message(&body), "Not found or no asset.download access");

    let _ = std::fs::remove_dir_all(&dir);
}
