use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
use image::{ImageFormat, Rgb, RgbImage};
use immich_server::{App, Config};
use serde_json::{json, Value};
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
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
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

fn upload(cookie: &str, bytes: &[u8], favorite: bool) -> Request<Body> {
    let boundary = "immichboundary";
    let favorite = if favorite { "true" } else { "false" };
    let mut body = Vec::new();
    for (name, value) in [
        ("deviceAssetId", "device-1"),
        ("deviceId", "phone"),
        ("fileCreatedAt", "2020-01-02T03:04:05.000Z"),
        ("fileModifiedAt", "2020-01-02T03:04:05.000Z"),
        ("isFavorite", favorite),
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
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("cookie", cookie)
        .body(Body::from(body))
        .unwrap()
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

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| panic!("body {}", String::from_utf8_lossy(bytes)))
}

fn lines_of(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8(bytes.to_vec())
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn types_of(lines: &[Value]) -> Vec<&str> {
    lines
        .iter()
        .map(|line| line["type"].as_str().unwrap())
        .collect()
}

async fn sign_in(app: &App, email: &str, password: &str) -> (String, String) {
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
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let login = json_body(&bytes);
    (cookie, login["userId"].as_str().unwrap().to_string())
}

async fn add_user(app: &App, cookie: &str, email: &str, name: &str) -> String {
    let body = format!(r#"{{"email":"{email}","password":"partner-secret","name":"{name}"}}"#);
    let (status, bytes) = call(
        app,
        json_request("POST", "/api/admin/users", Some(cookie), Some(&body)),
    )
    .await;
    let user = json_body(&bytes);
    assert_eq!(status, 201, "{user}");
    user["id"].as_str().unwrap().to_string()
}

async fn upload_asset(app: &App, cookie: &str, red: u8, favorite: bool) -> String {
    let (status, bytes) = call(app, upload(cookie, &jpeg(red), favorite)).await;
    let asset = json_body(&bytes);
    assert_eq!(status, 201, "{asset}");
    asset["id"].as_str().unwrap().to_string()
}

async fn sync(app: &App, cookie: &str, types: &str) -> Vec<Value> {
    let body = format!(r#"{{"types":{types}}}"#);
    let (status, bytes) = call(
        app,
        json_request("POST", "/api/sync/stream", Some(cookie), Some(&body)),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    lines_of(&bytes)
}

async fn ack(app: &App, cookie: &str, lines: &[Value]) {
    let acks: Vec<&str> = lines
        .iter()
        .filter_map(|line| line["ack"].as_str())
        .collect();
    let body = json!({ "acks": acks }).to_string();
    let (status, bytes) = call(
        app,
        json_request("POST", "/api/sync/ack", Some(cookie), Some(&body)),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
}

async fn bucket_ids(app: &App, cookie: &str, partners: &str) -> Vec<String> {
    let path = format!("/api/timeline/bucket?timeBucket=2020-01-01{partners}");
    let (status, bytes) = call(app, json_request("GET", &path, Some(cookie), None)).await;
    let bucket = json_body(&bytes);
    assert_eq!(status, 200, "{bucket}");
    bucket["id"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn share_a_library_with_a_partner() {
    let dir = std::env::temp_dir().join(format!("immich-partners-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let (ada, ada_id) = sign_in(&app, "admin@example.com", "correct horse").await;

    let (status, bytes) = call(&app, json_request("GET", "/api/partners", None, None)).await;
    let missing = json_body(&bytes);
    assert_eq!(status, 401, "{missing}");
    assert_eq!(missing["message"], "Authentication required");

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            "/api/partners?direction=shared-by",
            Some("immich_shared_link_token=link"),
            None,
        ),
    )
    .await;
    let shared = json_body(&bytes);
    assert_eq!(status, 403, "{shared}");
    assert_eq!(shared["message"], "Forbidden");

    let (status, bytes) = call(&app, json_request("GET", "/api/partners", Some(&ada), None)).await;
    assert_eq!(status, 400, "{}", json_body(&bytes));
    assert_eq!(json_body(&bytes)["message"], "Validation failed");

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/partners?direction=sideways", Some(&ada), None),
    )
    .await;
    assert_eq!(json_body(&bytes)["message"], "Validation failed");
    assert_eq!(status, 400);

    let bob_id = add_user(&app, &ada, "bob@example.com", "Bob").await;
    let cam_id = add_user(&app, &ada, "cam@example.com", "Cam").await;
    let ghost_id = add_user(&app, &ada, "ghost@example.com", "Ghost").await;
    let (status, bytes) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/admin/users/{ghost_id}"),
            Some(&ada),
            Some(r#"{"force":false}"#),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", json_body(&bytes));

    let (bob, _) = sign_in(&app, "bob@example.com", "partner-secret").await;
    let (cam, _) = sign_in(&app, "cam@example.com", "partner-secret").await;
    let ada_asset = upload_asset(&app, &ada, 10, false).await;
    let cam_old = upload_asset(&app, &cam, 20, false).await;
    let cam_older = upload_asset(&app, &cam, 30, false).await;
    let bob_asset = upload_asset(&app, &bob, 40, true).await;
    let cam_new = upload_asset(&app, &cam, 50, false).await;

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/partners",
            Some(&bob),
            Some(&format!(r#"{{"sharedWithId":"{ghost_id}"}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&bytes));
    assert_eq!(json_body(&bytes)["message"], "Invalid user");

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/partners",
            Some(&bob),
            Some(r#"{"sharedWithId":"not-a-uuid"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json_body(&bytes)["message"], "Validation failed");

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/partners",
            Some(&bob),
            Some(&format!(r#"{{"sharedWithId":"{ada_id}"}}"#)),
        ),
    )
    .await;
    let created = json_body(&bytes);
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["id"], ada_id);
    assert_eq!(created["email"], "admin@example.com");
    assert_eq!(created["name"], "Ada");
    assert_eq!(created["inTimeline"], false);
    assert!(!created["avatarColor"].as_str().unwrap().is_empty());
    assert!(created["profileChangedAt"].as_str().is_some());

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/partners",
            Some(&bob),
            Some(&format!(r#"{{"sharedWithId":"{ada_id}"}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json_body(&bytes)["message"], "Partner already exists");

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            "/api/partners?direction=shared-with",
            Some(&ada),
            None,
        ),
    )
    .await;
    let shared_with = json_body(&bytes);
    assert_eq!(status, 200, "{shared_with}");
    assert_eq!(shared_with.as_array().unwrap().len(), 1);
    assert_eq!(shared_with[0]["id"], bob_id);
    assert_eq!(shared_with[0]["inTimeline"], false);

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/partners?direction=shared-by", Some(&bob), None),
    )
    .await;
    let shared_by = json_body(&bytes);
    assert_eq!(status, 200, "{shared_by}");
    assert_eq!(shared_by[0]["id"], ada_id);

    let hidden = bucket_ids(&app, &ada, "&visibility=timeline&withPartners=true").await;
    assert!(hidden.contains(&ada_asset));
    assert!(!hidden.contains(&bob_asset));

    let own = bucket_ids(&app, &ada, "").await;
    assert!(!own.contains(&bob_asset));

    let (status, bytes) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/partners/{bob_id}"),
            Some(&ada),
            Some(r#"{"inTimeline":true}"#),
        ),
    )
    .await;
    let updated = json_body(&bytes);
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["id"], bob_id);
    assert_eq!(updated["inTimeline"], true);

    let (status, bytes) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/partners/{ada_id}"),
            Some(&bob),
            Some(r#"{"inTimeline":true}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&bytes));
    assert_eq!(
        json_body(&bytes)["message"],
        "Not found or no partner.update access"
    );

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            "/api/timeline/bucket?timeBucket=2020-01-01&withPartners=maybe",
            Some(&ada),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json_body(&bytes)["message"], "Validation failed");

    let visible = bucket_ids(&app, &ada, "&visibility=timeline&withPartners=true").await;
    assert!(visible.contains(&ada_asset));
    assert!(visible.contains(&bob_asset));
    let still_own = bucket_ids(&app, &ada, "&withPartners=false").await;
    assert!(!still_own.contains(&bob_asset));

    let ada_partners = sync(&app, &ada, r#"["PartnersV1"]"#).await;
    assert_eq!(types_of(&ada_partners), vec!["PartnerV1", "SyncCompleteV1"]);
    assert_eq!(ada_partners[0]["data"]["sharedById"], bob_id);
    assert_eq!(ada_partners[0]["data"]["sharedWithId"], ada_id);
    assert_eq!(ada_partners[0]["data"]["inTimeline"], true);
    ack(&app, &ada, &ada_partners).await;

    let bob_partners = sync(&app, &bob, r#"["PartnersV1"]"#).await;
    assert_eq!(bob_partners[0]["type"], "PartnerV1");
    assert_eq!(bob_partners[0]["data"]["inTimeline"], true);
    ack(&app, &bob, &bob_partners).await;

    let bob_assets = sync(&app, &bob, r#"["PartnerAssetsV2"]"#).await;
    assert_eq!(types_of(&bob_assets), vec!["SyncCompleteV1"]);

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/sync/stream",
            Some(&ada),
            Some(r#"{"types":["PartnerAssetsV1"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        json_body(&bytes)["message"],
        "SyncRequestType.PartnerAssetsV1 is deprecated, use SyncRequestType.PartnerAssetsV2 instead"
    );

    let first = sync(&app, &ada, r#"["PartnerAssetsV2","PartnerAssetExifsV1"]"#).await;
    assert_eq!(
        types_of(&first),
        vec!["PartnerAssetV2", "PartnerAssetExifV1", "SyncCompleteV1"]
    );
    assert_eq!(first[0]["data"]["id"], bob_asset);
    assert_eq!(first[0]["data"]["ownerId"], bob_id);
    assert_eq!(first[0]["data"]["isFavorite"], false);
    assert_eq!(first[1]["data"]["assetId"], bob_asset);
    ack(&app, &ada, &first).await;

    let (status, bytes) = call(
        &app,
        json_request("POST", &format!("/api/partners/{ada_id}"), Some(&cam), None),
    )
    .await;
    let deprecated = json_body(&bytes);
    assert_eq!(status, 201, "{deprecated}");
    assert_eq!(deprecated["id"], ada_id);
    assert_eq!(deprecated["inTimeline"], false);

    let backfill = sync(&app, &ada, r#"["PartnerAssetsV2"]"#).await;
    assert_eq!(
        types_of(&backfill),
        vec![
            "PartnerAssetBackfillV2",
            "PartnerAssetBackfillV2",
            "SyncAckV1",
            "PartnerAssetV2",
            "SyncCompleteV1",
        ]
    );
    assert_eq!(backfill[0]["data"]["id"], cam_old);
    assert_eq!(backfill[1]["data"]["id"], cam_older);
    assert_eq!(backfill[2]["data"], json!({}));
    assert!(backfill[2]["ack"].as_str().unwrap().ends_with("|complete"));
    assert_eq!(backfill[3]["data"]["id"], cam_new);
    assert_eq!(backfill[3]["data"]["isFavorite"], false);
    ack(&app, &ada, &backfill[..1]).await;

    let resumed = sync(&app, &ada, r#"["PartnerAssetsV2"]"#).await;
    assert_eq!(
        types_of(&resumed),
        vec![
            "PartnerAssetBackfillV2",
            "SyncAckV1",
            "PartnerAssetV2",
            "SyncCompleteV1",
        ]
    );
    assert_eq!(resumed[0]["data"]["id"], cam_older);
    assert_eq!(resumed[2]["data"]["id"], cam_new);
    ack(&app, &ada, &resumed).await;

    let visible = bucket_ids(&app, &ada, "&visibility=timeline&withPartners=true").await;
    assert!(visible.contains(&bob_asset));
    assert!(!visible.contains(&cam_old));

    let (status, _) = call(
        &app,
        json_request(
            "DELETE",
            "/api/assets",
            Some(&bob),
            Some(&format!(r#"{{"ids":["{bob_asset}"],"force":true}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 204);

    let deleted = sync(&app, &ada, r#"["PartnerAssetsV2"]"#).await;
    assert_eq!(
        types_of(&deleted),
        vec!["PartnerAssetDeleteV1", "SyncCompleteV1"]
    );
    assert_eq!(deleted[0]["data"]["assetId"], bob_asset);
    ack(&app, &ada, &deleted).await;

    let (status, bytes) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/partners/{ada_id}"),
            Some(&bob),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    assert!(bytes.is_empty());

    let (status, bytes) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/partners/{ada_id}"),
            Some(&bob),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json_body(&bytes)["message"], "Partner not found");

    let after_unshare = sync(&app, &ada, r#"["PartnerAssetsV2"]"#).await;
    assert_eq!(types_of(&after_unshare), vec!["SyncCompleteV1"]);

    let removed = sync(&app, &ada, r#"["PartnersV1"]"#).await;
    let removed_types = types_of(&removed);
    assert!(removed_types.contains(&"PartnerDeleteV1"));
    assert!(removed.iter().any(|line| {
        line["type"] == "PartnerDeleteV1"
            && line["data"]["sharedById"] == bob_id
            && line["data"]["sharedWithId"] == ada_id
    }));
    ack(&app, &ada, &removed).await;

    let (status, bytes) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/admin/users/{cam_id}"),
            Some(&ada),
            Some(r#"{"force":false}"#),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", json_body(&bytes));
    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            "/api/partners?direction=shared-with",
            Some(&ada),
            None,
        ),
    )
    .await;
    let remaining = json_body(&bytes);
    assert_eq!(status, 200, "{remaining}");
    assert!(remaining.as_array().unwrap().is_empty());

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/partners",
            Some(&ada),
            Some(&format!(r#"{{"sharedWithId":"{ada_id}"}}"#)),
        ),
    )
    .await;
    let self_share = json_body(&bytes);
    assert_eq!(status, 201, "{self_share}");
    assert_eq!(self_share["id"], ada_id);
    assert_eq!(self_share["inTimeline"], false);
    let (status, _) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/partners/{ada_id}"),
            Some(&ada),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204);

    let _ = std::fs::remove_dir_all(&dir);
}
