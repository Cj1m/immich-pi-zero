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
        machine_learning_enabled: true,
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
    let bytes = response.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, bytes)
}

fn jpeg(red: u8) -> Vec<u8> {
    let image = RgbImage::from_pixel(16, 10, Rgb([red, 40, 90]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn upload(cookie: &str, filename: &str, bytes: &[u8], created: &str) -> Request<Body> {
    let boundary = "immichboundary";
    let mut body = Vec::new();
    for (name, value) in [
        ("deviceAssetId", "device-1"),
        ("deviceId", "phone"),
        ("fileCreatedAt", created),
        ("fileModifiedAt", created),
        ("filename", filename),
    ] {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .into_bytes(),
        );
    }
    body.extend(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"{filename}\"\r\nContent-Type: image/jpeg\r\n\r\n")
            .into_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    Request::builder()
        .method("POST")
        .uri("/api/assets")
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .header("cookie", cookie)
        .body(Body::from(body))
        .unwrap()
}

fn json_request(method: &str, path: &str, cookie: Option<&str>, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder.body(Body::from(body.unwrap_or("").to_string())).unwrap()
}

#[tokio::test]
async fn nightly_memories_sync_and_the_web_list() {
    let dir = std::env::temp_dir().join(format!("immich-memories-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
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
    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("immich_access_token="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let older = jpeg(12);
    let newer = jpeg(80);
    let (status, body) = call(
        &app,
        upload(&cookie, "old.jpg", &older, "2018-05-04T15:00:00.000Z"),
    )
    .await;
    let created: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{created}");
    let older_id = created["id"].as_str().unwrap().to_string();
    let (status, body) = call(
        &app,
        upload(&cookie, "new.jpg", &newer, "2019-05-04T08:00:00.000Z"),
    )
    .await;
    let created: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{created}");
    let newer_id = created["id"].as_str().unwrap().to_string();

    app.generate_memories("2024-05-04").await.expect("generate");

    let (status, body) = call(&app, json_request("GET", "/api/memories", None, None)).await;
    let missing: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 401, "{missing}");
    assert_eq!(missing["message"], "Authentication required");

    let (status, body) = call(
        &app,
        json_request("GET", "/api/memories?for=2024-05-03", Some(&cookie), None),
    )
    .await;
    let outside: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{outside}");
    assert_eq!(outside.as_array().unwrap().len(), 0);

    let (status, body) = call(
        &app,
        json_request("GET", "/api/memories?for=2024-05-04&size=250&order=desc", Some(&cookie), None),
    )
    .await;
    let listed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed.as_array().unwrap().len(), 2);
    assert_eq!(listed[0]["type"], "on_this_day");
    assert_eq!(listed[0]["data"]["year"], 2019);
    assert_eq!(listed[0]["assets"][0]["id"], newer_id);
    assert_eq!(listed[0]["showAt"], "2024-05-04T00:00:00.000Z");
    assert_eq!(listed[0]["hideAt"], "2024-05-04T23:59:59.999Z");
    assert_eq!(listed[0]["isSaved"], false);
    assert!(listed[0]["seenAt"].is_null());
    assert_eq!(listed[1]["data"]["year"], 2018);
    assert_eq!(listed[1]["assets"][0]["id"], older_id);
    let newer_memory = listed[0]["id"].as_str().unwrap().to_string();
    let older_memory = listed[1]["id"].as_str().unwrap().to_string();

    let (status, body) = call(
        &app,
        json_request("GET", "/api/memories/statistics?for=2024-05-04", Some(&cookie), None),
    )
    .await;
    let stats: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{stats}");
    assert_eq!(stats["total"], 2);

    let (status, body) = call(
        &app,
        json_request("PUT", &format!("/api/memories/{newer_memory}"), Some(&cookie), Some("{}")),
    )
    .await;
    let empty: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{empty}");
    assert_eq!(empty["message"], "Validation failed");

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/memories/00000000-0000-4000-8000-000000000000",
            Some(&cookie),
            Some(r#"{"isSaved":true}"#),
        ),
    )
    .await;
    let absent: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{absent}");
    assert_eq!(absent["message"], "Not found or no memory.update access");

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/memories/{newer_memory}"),
            Some(&cookie),
            Some(r#"{"isSaved":true}"#),
        ),
    )
    .await;
    let saved: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["isSaved"], true);

    let (status, body) = call(
        &app,
        json_request("POST", "/api/sync/stream", Some(&cookie), Some(
            r#"{"types":["MemoriesV2","MemoryToAssetsV2"]}"#,
        )),
    )
    .await;
    assert_eq!(status, 200);
    let lines = sync_lines(&body);
    assert!(lines.iter().any(|line| line["type"] == "MemoryV2" && line["data"]["id"] == newer_memory && line["data"]["isSaved"] == true));
    assert!(lines.iter().any(|line| line["type"] == "MemoryToAssetV2" && line["data"]["memoryId"] == older_memory && line["data"]["assetId"] == older_id));

    let (status, link_body) = call(
        &app,
        json_request(
            "POST",
            "/api/shared-links",
            Some(&cookie),
            Some(&format!(r#"{{"type":"INDIVIDUAL","assetIds":["{newer_id}"]}}"#)),
        ),
    )
    .await;
    let link: Value = serde_json::from_slice(&link_body).unwrap();
    assert_eq!(status, 201, "{link}");
    let key = link["key"].as_str().unwrap();
    let (status, body) = call(
        &app,
        Request::builder()
            .method("GET")
            .uri("/api/memories")
            .header("x-immich-share-key", key)
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let forbidden: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 403, "{forbidden}");
    assert_eq!(forbidden["message"], "Forbidden");

    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/memories/{older_memory}/assets"),
            Some(&cookie),
            Some(&format!(r#"{{"ids":["{older_id}","00000000-0000-4000-8000-000000000001"]}}"#)),
        ),
    )
    .await;
    let removed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{removed}");
    assert_eq!(removed[0]["success"], true);
    assert_eq!(removed[1]["error"], "not_found");

    sqlx::query("UPDATE memory SET createdAt = ? WHERE id = ?")
        .bind("2020-01-01T00:00:00.000Z")
        .bind(&older_memory)
        .execute(app.pool())
        .await
        .unwrap();
    app.cleanup_memories("2024-06-04T00:00:00.000Z").await.expect("cleanup");
    let still_saved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory WHERE id = ?")
        .bind(&newer_memory)
        .fetch_one(app.pool())
        .await
        .unwrap();
    let removed_old: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory WHERE id = ?")
        .bind(&older_memory)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(still_saved, 1);
    assert_eq!(removed_old, 0);

    let (status, body) = call(
        &app,
        json_request("DELETE", &format!("/api/memories/{newer_memory}"), Some(&cookie), None),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));
    assert!(body.is_empty());

    let (status, body) = call(
        &app,
        json_request("GET", "/api/memories", Some(&cookie), None),
    )
    .await;
    let empty_list: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{empty_list}");
    assert_eq!(empty_list.as_array().unwrap().len(), 0);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/sync/stream",
            Some(&cookie),
            Some(r#"{"types":["MemoriesV2","MemoryToAssetsV2"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let lines = sync_lines(&body);
    assert!(lines.iter().any(|line| line["type"] == "MemoryDeleteV1" && line["data"]["memoryId"] == newer_memory));
    assert!(lines.iter().any(|line| line["type"] == "MemoryToAssetDeleteV1" && line["data"]["assetId"] == older_id));

    let _ = std::fs::remove_dir_all(&dir);
}

fn sync_lines(body: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(body)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
