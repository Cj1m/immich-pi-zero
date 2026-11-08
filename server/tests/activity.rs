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
    let response = immich_server::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, bytes)
}

fn jpeg() -> Vec<u8> {
    let image = RgbImage::from_pixel(16, 10, Rgb([20, 40, 90]));
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
        ("deviceAssetId", "device-1"),
        ("deviceId", "phone"),
        ("fileCreatedAt", "2020-01-02T03:04:05.000Z"),
        ("fileModifiedAt", "2020-01-02T03:04:05.000Z"),
        ("filename", "cat.jpg"),
    ] {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").into_bytes(),
        );
    }
    body.extend(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"cat.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n")
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
async fn album_activity_lists_likes_and_comments() {
    let dir = std::env::temp_dir().join(format!("immich-activity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let response = immich_server::router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"admin@example.com","password":"correct horse"}"#))
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

    let (status, body) = call(&app, upload(&cookie, &jpeg())).await;
    let asset: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{asset}");
    let asset_id = asset["id"].as_str().unwrap();

    let (status, body) = call(
        &app,
        json_request("POST", "/api/albums", Some(&cookie), Some(r#"{"albumName":"Trip"}"#)),
    )
    .await;
    let album: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{album}");
    let album_id = album["id"].as_str().unwrap();
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/albums/{album_id}/assets"),
            Some(&cookie),
            Some(&format!(r#"{{"ids":["{asset_id}"]}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));

    let (status, body) = call(&app, json_request("GET", "/api/activities", None, None)).await;
    let missing: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 401, "{missing}");
    assert_eq!(missing["message"], "Authentication required");

    let (status, body) = call(
        &app,
        json_request("GET", "/api/activities", Some(&cookie), None),
    )
    .await;
    let invalid: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{invalid}");
    assert_eq!(invalid["message"], "Validation failed");

    let list_path = format!("/api/activities?albumId={album_id}");
    let (status, body) = call(&app, json_request("GET", &list_path, Some(&cookie), None)).await;
    let empty: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{empty}");
    assert_eq!(empty.as_array().unwrap().len(), 0);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/activities",
            Some(&cookie),
            Some(&format!(r#"{{"albumId":"{album_id}","type":"like"}}"#)),
        ),
    )
    .await;
    let album_like: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{album_like}");
    assert_eq!(album_like["type"], "like");
    assert!(album_like["assetId"].is_null());
    assert!(album_like["comment"].is_null());
    assert_eq!(album_like["user"]["email"], "admin@example.com");
    let album_like_id = album_like["id"].as_str().unwrap();

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/activities",
            Some(&cookie),
            Some(&format!(r#"{{"albumId":"{album_id}","type":"like"}}"#)),
        ),
    )
    .await;
    let duplicate: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{duplicate}");
    assert_eq!(duplicate["id"], album_like_id);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/activities",
            Some(&cookie),
            Some(&format!(r#"{{"albumId":"{album_id}","assetId":"{asset_id}","type":"comment","comment":"  "}}"#)),
        ),
    )
    .await;
    let blank: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{blank}");
    assert_eq!(blank["message"], "Validation failed");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/activities",
            Some(&cookie),
            Some(&format!(
                r#"{{"albumId":"{album_id}","assetId":"{asset_id}","type":"comment","comment":"hello"}}"#
            )),
        ),
    )
    .await;
    let comment: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{comment}");
    assert_eq!(comment["type"], "comment");
    assert_eq!(comment["comment"], "hello");
    assert_eq!(comment["assetId"], asset_id);
    let comment_id = comment["id"].as_str().unwrap().to_string();

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/activities",
            Some(&cookie),
            Some(&format!(r#"{{"albumId":"{album_id}","assetId":"{asset_id}","type":"like"}}"#)),
        ),
    )
    .await;
    let asset_like: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{asset_like}");
    assert_eq!(asset_like["assetId"], asset_id);

    let (status, body) = call(
        &app,
        json_request("GET", &format!("/api/activities/statistics?albumId={album_id}"), Some(&cookie), None),
    )
    .await;
    let stats: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{stats}");
    assert_eq!(stats["comments"], 1);
    assert_eq!(stats["likes"], 2);

    let (status, _) = call(
        &app,
        json_request("DELETE", "/api/assets", Some(&cookie), Some(&format!(r#"{{"ids":["{asset_id}"]}}"#))),
    )
    .await;
    assert_eq!(status, 204);
    let (status, body) = call(&app, json_request("GET", &list_path, Some(&cookie), None)).await;
    let trashed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{trashed}");
    assert_eq!(trashed.as_array().unwrap().len(), 1);
    assert_eq!(trashed[0]["id"], album_like_id);
    let (status, body) = call(
        &app,
        json_request("GET", &format!("/api/activities/statistics?albumId={album_id}"), Some(&cookie), None),
    )
    .await;
    let hidden: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(hidden["comments"], 0);
    assert_eq!(hidden["likes"], 1);
    assert_eq!(status, 200);

    let (status, _) = call(
        &app,
        json_request(
            "POST",
            "/api/trash/restore/assets",
            Some(&cookie),
            Some(&format!(r#"{{"ids":["{asset_id}"]}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = call(&app, json_request("GET", &list_path, Some(&cookie), None)).await;
    let restored: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{restored}");
    assert_eq!(restored.as_array().unwrap().len(), 3);

    let (status, body) = call(
        &app,
        json_request("DELETE", &format!("/api/activities/{comment_id}"), Some(&cookie), None),
    )
    .await;
    assert_eq!(status, 204);
    assert!(body.is_empty());
    let (status, body) = call(
        &app,
        json_request("DELETE", &format!("/api/activities/{comment_id}"), Some(&cookie), None),
    )
    .await;
    let gone: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{gone}");
    assert_eq!(gone["message"], "Not found or no activity.delete access");

    let (status, _) = call(
        &app,
        json_request(
            "PATCH",
            &format!("/api/albums/{album_id}"),
            Some(&cookie),
            Some(r#"{"isActivityEnabled":false}"#),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/activities",
            Some(&cookie),
            Some(&format!(r#"{{"albumId":"{album_id}","type":"comment","comment":"later"}}"#)),
        ),
    )
    .await;
    let closed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{closed}");
    assert_eq!(closed["message"], "Not found or no activity.create access");
    let (status, body) = call(&app, json_request("GET", &list_path, Some(&cookie), None)).await;
    let still: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{still}");
    assert_eq!(still.as_array().unwrap().len(), 2);

    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/albums/{album_id}/assets"),
            Some(&cookie),
            Some(&format!(r#"{{"ids":["{asset_id}"]}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let (status, body) = call(&app, json_request("GET", &list_path, Some(&cookie), None)).await;
    let unlinked: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{unlinked}");
    assert_eq!(unlinked.as_array().unwrap().len(), 1);
    assert_eq!(unlinked[0]["id"], album_like_id);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/shared-links",
            Some(&cookie),
            Some(&format!(r#"{{"type":"INDIVIDUAL","assetIds":["{asset_id}"]}}"#)),
        ),
    )
    .await;
    let link: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{link}");
    let key = link["key"].as_str().unwrap();
    let (status, body) = call(
        &app,
        Request::builder()
            .method("GET")
            .uri(&list_path)
            .header("x-immich-share-key", key)
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let forbidden: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 403, "{forbidden}");
    assert_eq!(forbidden["message"], "Forbidden");

    let _ = std::fs::remove_dir_all(&dir);
}
