use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
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
        machine_learning_enabled: true,
        config_file: None,
        log_level: None,
    }
}

async fn session(dir: &Path) -> (App, String) {
    let app = App::prepare(&config(dir)).await.expect("prepare");
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
    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("immich_access_token="))
        .expect("cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();
    (app, cookie)
}

async fn call(app: &App, request: Request<Body>) -> (http::StatusCode, Vec<u8>, Option<String>) {
    let response = immich_server::router(app.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes, content_type)
}

fn jpeg(red: u8) -> Vec<u8> {
    jpeg_size(16, 10, red)
}

fn jpeg_size(width: u32, height: u32, red: u8) -> Vec<u8> {
    let image = RgbImage::from_pixel(width, height, Rgb([red, 40, 90]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn multipart(filename: &str, bytes: &[u8], extra: &[(&str, &str)]) -> (String, Vec<u8>) {
    let boundary = "immichboundary";
    let mut body = Vec::new();
    for (name, value) in extra {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").into_bytes());
    }
    body.extend(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"{filename}\"\r\nContent-Type: image/jpeg\r\n\r\n")
            .into_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn upload_request(cookie: &str, filename: &str, bytes: &[u8], created: &str) -> Request<Body> {
    let (content_type, body) = multipart(
        filename,
        bytes,
        &[
            ("deviceAssetId", "device-1"),
            ("deviceId", "phone"),
            ("fileCreatedAt", created),
            ("fileModifiedAt", created),
            ("filename", filename),
        ],
    );
    Request::builder()
        .method("POST")
        .uri("/api/assets")
        .header("content-type", content_type)
        .header("cookie", cookie)
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn upload_syncs_and_albums_keep_the_asset_after_delete() {
    let dir = std::env::temp_dir().join(format!("immich-library-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (app, cookie) = session(&dir).await;
    let bytes = jpeg(12);
    let created = "2020-05-04T03:02:01.000Z";

    let (status, body, _) = call(&app, upload_request(&cookie, "cat.jpg", &bytes, created)).await;
    let created_body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{created_body}");
    assert_eq!(created_body["status"], "created");
    let id = created_body["id"].as_str().unwrap().to_string();

    let (status, body, _) = call(&app, upload_request(&cookie, "cat.jpg", &bytes, created)).await;
    let duplicate: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{duplicate}");
    assert_eq!(duplicate["status"], "duplicate");
    assert_eq!(duplicate["id"], id);

    let (status, body, _) = call(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/api/assets/{id}"))
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let asset: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{asset}");
    assert_eq!(asset["type"], "IMAGE");
    assert!(asset["libraryId"].is_null());
    assert_eq!(asset["checksum"].as_str().unwrap().len(), 28);
    assert_eq!(asset["width"], 16);
    assert_eq!(asset["height"], 10);
    assert_eq!(asset["fileCreatedAt"], created);
    assert_eq!(asset["localDateTime"], created);
    assert!(asset["exifInfo"]["make"].is_null());
    assert!(asset["exifInfo"]["dateTimeOriginal"].is_null());
    assert!(asset["exifInfo"]["latitude"].is_null());
    assert!(asset["duration"].is_null());
    let expected = base64_sha1(&bytes);
    assert_eq!(asset["checksum"], expected);

    let (status, original, content_type) = call(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/api/assets/{id}/original"))
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(original, bytes);
    assert_eq!(content_type.as_deref(), Some("image/jpeg"));

    let (status, ranged, _) = call(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/api/assets/{id}/original"))
            .header("cookie", &cookie)
            .header("range", "bytes=0-3")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 206);
    assert_eq!(ranged, bytes[..4]);

    let (status, thumb, content_type) = call(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/api/assets/{id}/thumbnail?size=thumbnail"))
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&thumb));
    assert_eq!(content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(&thumb[..2], &[0xFF, 0xD8]);

    let (status, stream, content_type) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/sync/stream")
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"types":["AuthUsersV2","AssetsV2","AssetExifsV1","AlbumsV2","AlbumUsersV1","AlbumToAssetsV1"]}"#,
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(content_type.as_deref(), Some("application/jsonlines+json"));
    let lines: Vec<Value> = String::from_utf8(stream)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let asset_line = lines.iter().find(|line| line["type"] == "AssetV2").unwrap();
    assert_eq!(asset_line["data"]["id"], id);
    assert_eq!(asset_line["data"]["duration"], Value::Null);
    assert_eq!(asset_line["data"]["checksum"], expected);
    assert_eq!(asset_line["data"]["type"], "IMAGE");
    assert!(lines.iter().any(|line| line["type"] == "SyncCompleteV1"));
    let ack = asset_line["ack"].as_str().unwrap();

    let (status, _, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/sync/ack")
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from(format!(r#"{{"acks":["{ack}"]}}"#)))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 204);
    let (status, stream, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/sync/stream")
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from(r#"{"types":["AssetsV2"]}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    let second = String::from_utf8(stream).unwrap();
    assert!(!second.contains("AssetV2"));
    assert!(second.contains("SyncCompleteV1"));

    let (status, album_body, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/albums")
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from(format!(
                r#"{{"albumName":"Trip","description":null,"assetIds":["{id}"]}}"#
            )))
            .unwrap(),
    )
    .await;
    let album: Value = serde_json::from_slice(&album_body).unwrap();
    assert_eq!(status, 201, "{album}");
    assert_eq!(album["albumName"], "Trip");
    assert_eq!(album["description"], "");
    assert_eq!(album["assetCount"], 1);
    assert_eq!(album["albumThumbnailAssetId"], id);
    assert_eq!(album["albumUsers"][0]["role"], "owner");
    assert!(album["albumUsers"][0]["user"]["email"]
        .as_str()
        .unwrap()
        .contains('@'));
    let album_id = album["id"].as_str().unwrap();

    let (status, added, _) = call(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/api/albums/{album_id}/assets"))
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from(format!(r#"{{"ids":["{id}"]}}"#)))
            .unwrap(),
    )
    .await;
    let added: Value = serde_json::from_slice(&added).unwrap();
    assert_eq!(status, 200, "{added}");
    assert_eq!(added[0]["success"], false);
    assert_eq!(added[0]["error"], "duplicate");

    let (status, _, _) = call(
        &app,
        Request::builder()
            .method("DELETE")
            .uri("/api/assets")
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from(format!(r#"{{"ids":["{id}"],"force":false}}"#)))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 204);
    let (status, again, _) = call(&app, upload_request(&cookie, "cat.jpg", &bytes, created)).await;
    let again: Value = serde_json::from_slice(&again).unwrap();
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["id"], id);

    let (status, _, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/trash/restore/assets")
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from(format!(r#"{{"ids":["{id}"]}}"#)))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);

    let (status, _, _) = call(
        &app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/albums/{album_id}"))
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 204);
    let (status, _, _) = call(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/api/assets/{id}"))
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);

    let other = jpeg(200);
    let (status_a, body_a, _) = call(&app, upload_request(&cookie, "a.jpg", &other, created)).await;
    let second_bytes = jpeg(80);
    let (status_b, body_b, _) = call(
        &app,
        upload_request(&cookie, "b.jpg", &second_bytes, created),
    )
    .await;
    assert_eq!(status_a, 201);
    assert_eq!(status_b, 201);
    let first_id = serde_json::from_slice::<Value>(&body_a).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let second_id = serde_json::from_slice::<Value>(&body_b).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, stack, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/stacks")
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from(format!(
                r#"{{"assetIds":["{first_id}","{second_id}"]}}"#
            )))
            .unwrap(),
    )
    .await;
    let stack: Value = serde_json::from_slice(&stack).unwrap();
    assert_eq!(status, 201, "{stack}");
    assert!(stack["id"].is_string());

    let exif_bytes = immich_server::embed_exif(
        &jpeg_size(32, 8, 30),
        &immich_server::ExifFixture {
            make: Some("Canon".to_string()),
            model: Some("EOS R6".to_string()),
            lens_model: Some("RF 50mm".to_string()),
            description: Some("A cat".to_string()),
            orientation: Some(6),
            rating: Some(4),
            date_time_original: Some("2020:05:04 03:02:01".to_string()),
            offset_time_original: Some("+02:00".to_string()),
            iso: Some(200),
            exposure: Some((1, 125)),
            f_number: Some((28, 10)),
            focal_length: Some((50, 1)),
            latitude: Some(37.5),
            longitude: Some(-122.5),
            ..immich_server::ExifFixture::default()
        },
    );
    let (status, body, _) = call(
        &app,
        upload_request(&cookie, "exif.jpg", &exif_bytes, created),
    )
    .await;
    let uploaded: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{uploaded}");
    let exif_id = uploaded["id"].as_str().unwrap();
    let (status, body, _) = call(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/api/assets/{exif_id}"))
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let detailed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{detailed}");
    assert_eq!(detailed["width"], 8);
    assert_eq!(detailed["height"], 32);
    assert_eq!(detailed["fileCreatedAt"], "2020-05-04T01:02:01.000Z");
    assert_eq!(detailed["localDateTime"], "2020-05-04T03:02:01.000Z");
    assert_eq!(detailed["fileModifiedAt"], created);
    let info = &detailed["exifInfo"];
    assert_eq!(info["make"], "Canon");
    assert_eq!(info["model"], "EOS R6");
    assert_eq!(info["lensModel"], "RF 50mm");
    assert_eq!(info["description"], "A cat");
    assert_eq!(info["orientation"], "6");
    assert_eq!(info["rating"], 4);
    assert_eq!(info["dateTimeOriginal"], "2020-05-04T01:02:01.000Z");
    assert_eq!(info["timeZone"], "+02:00");
    assert_eq!(info["iso"], 200);
    assert_eq!(info["exposureTime"], "1/125");
    assert_eq!(info["focalLength"], 50.0);
    let aperture = info["fNumber"].as_f64().unwrap();
    assert!((aperture - 2.8).abs() < 0.001, "{aperture}");
    assert_eq!(info["latitude"], 37.5);
    assert_eq!(info["longitude"], -122.5);
    assert!(info["city"].is_null());
    assert!(info["state"].is_null());
    assert!(info["country"].is_null());
    assert_eq!(info["exifImageWidth"], 32);
    assert_eq!(info["exifImageHeight"], 8);

    let (status, original, _) = call(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/api/assets/{exif_id}/original"))
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(original, exif_bytes);

    let (status, thumb, _) = call(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/api/assets/{exif_id}/thumbnail?size=preview"))
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    let preview = image::ImageReader::new(std::io::Cursor::new(thumb))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap();
    assert!(preview.height() > preview.width());

    let _ = std::fs::remove_dir_all(&dir);
}

fn base64_sha1(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(Sha1::digest(bytes))
}
