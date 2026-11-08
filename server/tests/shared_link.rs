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

struct Reply {
    status: http::StatusCode,
    body: Vec<u8>,
    headers: http::HeaderMap,
}

async fn call(app: &App, request: Request<Body>) -> Reply {
    let response = immich_server::router(app.clone())
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    Reply {
        status,
        body,
        headers,
    }
}

fn json_body(reply: &Reply) -> Value {
    serde_json::from_slice(&reply.body).unwrap_or_else(|_| {
        panic!(
            "status {} body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        )
    })
}

fn text(reply: &Reply) -> String {
    String::from_utf8_lossy(&reply.body).into_owned()
}

fn cookie_pair(headers: &http::HeaderMap, name: &str) -> String {
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(name))
        .unwrap_or_else(|| panic!("missing {name}"))
        .split(';')
        .next()
        .expect("cookie pair")
        .to_string()
}

fn request(method: &str, path: &str, cookie: Option<&str>, body: Option<&str>) -> Request<Body> {
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

fn jpeg() -> Vec<u8> {
    let image = RgbImage::from_pixel(16, 10, Rgb([12, 40, 90]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn upload(cookie: &str, bytes: &[u8], sidecar: &[u8]) -> Request<Body> {
    let boundary = "immichboundary";
    let created = "2020-05-04T03:02:01.000Z";
    let mut body = Vec::new();
    for (name, value) in [
        ("deviceAssetId", "device-1"),
        ("deviceId", "phone"),
        ("fileCreatedAt", created),
        ("fileModifiedAt", created),
        ("filename", "cat.jpg"),
    ] {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .into_bytes(),
        );
    }
    body.extend(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"sidecarData\"; filename=\"cat.jpg.xmp\"\r\nContent-Type: application/xml\r\n\r\n")
            .into_bytes(),
    );
    body.extend_from_slice(sidecar);
    body.extend(
        format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"cat.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n")
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

#[tokio::test]
async fn shared_link_opens_the_album_and_downloads_originals() {
    let dir = std::env::temp_dir().join(format!("immich-share-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("web")).unwrap();
    std::fs::write(
        dir.join("web/index.html"),
        "<!doctype html><head><!-- metadata:tags --></head>",
    )
    .unwrap();
    std::fs::write(dir.join("web/plain.txt"), "hello web").unwrap();

    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let signed_in = call(
        &app,
        request(
            "POST",
            "/api/auth/login",
            None,
            Some(r#"{"email":"admin@example.com","password":"correct horse"}"#),
        ),
    )
    .await;
    assert_eq!(signed_in.status, 201);
    let session = cookie_pair(&signed_in.headers, "immich_access_token=");

    let photo = jpeg();
    let sidecar = b"<x:xmpmeta>sidecar</x:xmpmeta>";
    let uploaded = call(&app, upload(&session, &photo, sidecar)).await;
    let created = json_body(&uploaded);
    assert_eq!(uploaded.status, 201, "{created}");
    let asset_id = created["id"].as_str().unwrap().to_string();

    let made = call(
        &app,
        request(
            "POST",
            "/api/shared-links",
            Some(&session),
            Some(&format!(
                r#"{{"type":"INDIVIDUAL","assetIds":["{asset_id}"],"slug":"summer","description":"cats"}}"#
            )),
        ),
    )
    .await;
    let link = json_body(&made);
    assert_eq!(made.status, 201, "{link}");
    let link_id = link["id"].as_str().unwrap().to_string();
    let key = link["key"].as_str().unwrap().to_string();
    assert_eq!(link["assets"][0]["id"], asset_id);
    assert_eq!(link["slug"], "summer");
    assert!(link["allowDownload"].as_bool().unwrap());

    let listed = call(
        &app,
        request("GET", "/api/shared-links", Some(&session), None),
    )
    .await;
    let listed_body = json_body(&listed);
    assert_eq!(listed.status, 200, "{listed_body}");
    assert_eq!(listed_body[0]["id"], link_id);

    let opened = call(
        &app,
        request(
            "GET",
            &format!("/api/shared-links/me?key={key}"),
            None,
            None,
        ),
    )
    .await;
    let opened_body = json_body(&opened);
    assert_eq!(opened.status, 200, "{opened_body}");
    assert_eq!(opened_body["assets"][0]["id"], asset_id);
    assert_eq!(opened_body["assets"][0]["people"], json!([]));

    let thumb = call(
        &app,
        request(
            "GET",
            &format!("/api/assets/{asset_id}/thumbnail?size=thumbnail&key={key}"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(thumb.status, 200);
    assert_eq!(&thumb.body[..2], b"\xff\xd8");

    let original = call(
        &app,
        request(
            "GET",
            &format!("/api/assets/{asset_id}/original?key={key}"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(original.status, 200);
    assert_eq!(original.body, photo);

    let patched = call(
        &app,
        request(
            "PATCH",
            &format!("/api/shared-links/{link_id}"),
            Some(&session),
            Some(r#"{"description":"renamed"}"#),
        ),
    )
    .await;
    let patched_body = json_body(&patched);
    assert_eq!(patched.status, 200, "{patched_body}");
    assert!(patched_body["slug"].is_null(), "{patched_body}");
    assert_eq!(patched_body["description"], "renamed");

    let info = call(
        &app,
        request(
            "POST",
            "/api/download/info",
            Some(&session),
            Some(&format!(r#"{{"assetIds":["{asset_id}"]}}"#)),
        ),
    )
    .await;
    let info_body = json_body(&info);
    assert_eq!(info.status, 201, "{info_body}");
    assert_eq!(info_body["archives"][0]["assetIds"][0], asset_id);

    let archive = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/download/archive")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", &session)
            .body(Body::from(format!(
                "assetIds={asset_id}&archiveName=Summer"
            )))
            .unwrap(),
    )
    .await;
    assert_eq!(archive.status, 200, "{}", text(&archive));
    assert_eq!(&archive.body[..2], b"PK");
    assert!(archive.body.windows(7).any(|window| window == b"cat.jpg"));
    assert!(!archive.body.windows(4).any(|window| window == b".xmp"));
    assert!(!archive.body.windows(8).any(|window| window == b"xmpmeta"));
    let disposition = archive
        .headers
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert!(
        disposition.contains("filename*=UTF-8''Summer.zip"),
        "{disposition}"
    );

    let album = call(
        &app,
        request(
            "POST",
            "/api/albums",
            Some(&session),
            Some(&format!(
                r#"{{"albumName":"Vacation","assetIds":["{asset_id}"]}}"#
            )),
        ),
    )
    .await;
    let album_body = json_body(&album);
    assert_eq!(album.status, 201, "{album_body}");
    let album_id = album_body["id"].as_str().unwrap().to_string();

    let album_link = call(
        &app,
        request(
            "POST",
            "/api/shared-links",
            Some(&session),
            Some(&format!(
                r#"{{"type":"ALBUM","albumId":"{album_id}","slug":"vacation","description":"the trip"}}"#
            )),
        ),
    )
    .await;
    let album_link_body = json_body(&album_link);
    assert_eq!(album_link.status, 201, "{album_link_body}");
    let album_key = album_link_body["key"].as_str().unwrap().to_string();
    assert_eq!(album_link_body["album"]["albumName"], "Vacation");
    assert_eq!(album_link_body["album"]["hasSharedLink"], true);
    assert_eq!(album_link_body["album"]["shared"], true);

    let album_row = call(
        &app,
        request(
            "GET",
            &format!("/api/albums/{album_id}"),
            Some(&session),
            None,
        ),
    )
    .await;
    let album_row_body = json_body(&album_row);
    assert_eq!(album_row.status, 200, "{album_row_body}");
    assert_eq!(album_row_body["hasSharedLink"], true);

    let page = call(
        &app,
        Request::builder()
            .method("GET")
            .uri("/s/vacation")
            .header("accept", "text/html")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let page_html = text(&page);
    assert_eq!(page.status, 200, "{page_html}");
    assert!(page_html.contains("og:title"), "{page_html}");
    assert!(page_html.contains("Vacation"), "{page_html}");
    assert_eq!(
        page.headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );

    let missing = call(
        &app,
        Request::builder()
            .method("GET")
            .uri("/s/not-a-real-slug")
            .header("accept", "text/html")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(missing.status, 404);
    assert!(text(&missing).contains("<!-- metadata:tags -->"));

    let refused = call(
        &app,
        Request::builder()
            .method("GET")
            .uri("/s/vacation")
            .header("accept", "application/json")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let refused_body = json_body(&refused);
    assert_eq!(refused.status, 406, "{refused_body}");
    assert!(refused_body["message"]
        .as_str()
        .unwrap()
        .contains("only returns text/html"));

    let hosted = call(
        &app,
        Request::builder()
            .uri("/plain.txt")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(hosted.status, 200);
    assert_eq!(hosted.body, b"hello web");

    let unknown = call(
        &app,
        Request::builder()
            .uri("/api/does-not-exist")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let unknown_body = json_body(&unknown);
    assert_eq!(unknown.status, 404, "{unknown_body}");
    assert_eq!(unknown_body["message"], "Cannot GET /api/does-not-exist");

    let types = call(&app, request("GET", "/api/server/media-types", None, None)).await;
    let types_body = json_body(&types);
    assert_eq!(types.status, 200, "{types_body}");
    let contains = |name: &str, extension: &str| {
        types_body[name]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some(extension))
    };
    assert!(contains("image", ".jpg"));
    assert!(contains("video", ".mp4"));
    assert!(contains("sidecar", ".xmp"));

    let buckets = call(
        &app,
        request(
            "GET",
            &format!("/api/timeline/buckets?albumId={album_id}&key={album_key}"),
            None,
            None,
        ),
    )
    .await;
    let buckets_body = json_body(&buckets);
    assert_eq!(buckets.status, 200, "{buckets_body}");
    assert_eq!(buckets_body[0]["timeBucket"], "2020-05-01");
    assert!(buckets_body[0]["count"].as_i64().unwrap() >= 1);

    let bucket = call(
        &app,
        request(
            "GET",
            &format!(
                "/api/timeline/bucket?albumId={album_id}&key={album_key}&timeBucket=2020-05-01T00:00:00.000Z"
            ),
            None,
            None,
        ),
    )
    .await;
    let bucket_body = json_body(&bucket);
    assert_eq!(bucket.status, 200, "{bucket_body}");
    assert_eq!(bucket_body["id"][0], asset_id);

    let placed = call(
        &app,
        request(
            "PUT",
            &format!("/api/assets/{asset_id}"),
            Some(&session),
            Some(r#"{"latitude":37.5,"longitude":-122.5}"#),
        ),
    )
    .await;
    assert_eq!(placed.status, 200, "{}", text(&placed));

    let markers = call(
        &app,
        request(
            "GET",
            &format!("/api/albums/{album_id}/map-markers?key={album_key}"),
            None,
            None,
        ),
    )
    .await;
    let markers_body = json_body(&markers);
    assert_eq!(markers.status, 200, "{markers_body}");
    assert_eq!(markers_body[0]["id"], asset_id);
    assert_eq!(markers_body[0]["lat"].as_f64(), Some(37.5));
    assert_eq!(markers_body[0]["lon"].as_f64(), Some(-122.5));

    let locked = call(
        &app,
        request(
            "POST",
            "/api/shared-links",
            Some(&session),
            Some(&format!(
                r#"{{"type":"ALBUM","albumId":"{album_id}","slug":"locked","password":"secret"}}"#
            )),
        ),
    )
    .await;
    let locked_body = json_body(&locked);
    assert_eq!(locked.status, 201, "{locked_body}");
    let locked_key = locked_body["key"].as_str().unwrap();
    assert_eq!(locked_body["password"], "secret");

    let gated = call(
        &app,
        request(
            "GET",
            &format!("/api/shared-links/me?key={locked_key}"),
            None,
            None,
        ),
    )
    .await;
    let gated_body = json_body(&gated);
    assert_eq!(gated.status, 401, "{gated_body}");
    assert_eq!(gated_body["message"], "Password required");

    let wrong = call(
        &app,
        request(
            "POST",
            &format!("/api/shared-links/login?key={locked_key}"),
            None,
            Some(r#"{"password":"nope"}"#),
        ),
    )
    .await;
    let wrong_body = json_body(&wrong);
    assert_eq!(wrong.status, 401, "{wrong_body}");
    assert_eq!(wrong_body["message"], "Invalid password");

    let right = call(
        &app,
        request(
            "POST",
            &format!("/api/shared-links/login?key={locked_key}"),
            None,
            Some(r#"{"password":"secret"}"#),
        ),
    )
    .await;
    let right_body = json_body(&right);
    assert_eq!(right.status, 201, "{right_body}");
    let share_cookie = cookie_pair(&right.headers, "immich_shared_link_token=");
    assert!(share_cookie.contains("immich_shared_link_token="));

    let unlocked = call(
        &app,
        request(
            "GET",
            &format!("/api/shared-links/me?key={locked_key}"),
            Some(&share_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(unlocked.status, 200, "{}", text(&unlocked));

    let quiet = call(
        &app,
        Request::builder()
            .uri("/s/locked")
            .header("accept", "text/html")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(quiet.status, 200);
    assert!(text(&quiet).contains("<!-- metadata:tags -->"));

    let unprotected = call(
        &app,
        request(
            "POST",
            &format!("/api/shared-links/login?key={album_key}"),
            None,
            Some(r#"{"password":"secret"}"#),
        ),
    )
    .await;
    let unprotected_body = json_body(&unprotected);
    assert_eq!(unprotected.status, 400, "{unprotected_body}");
    assert_eq!(
        unprotected_body["message"],
        "Shared link is not password protected"
    );

    let bad_key = call(
        &app,
        request("GET", "/api/shared-links/me?key=not-a-key", None, None),
    )
    .await;
    let bad_key_body = json_body(&bad_key);
    assert_eq!(bad_key.status, 401, "{bad_key_body}");
    assert_eq!(bad_key_body["message"], "Invalid share key");

    let hidden = call(
        &app,
        request(
            "POST",
            "/api/shared-links",
            Some(&session),
            Some(&format!(
                r#"{{"type":"INDIVIDUAL","assetIds":["{asset_id}"],"showMetadata":false}}"#
            )),
        ),
    )
    .await;
    let hidden_body = json_body(&hidden);
    assert_eq!(hidden.status, 201, "{hidden_body}");
    assert_eq!(hidden_body["showMetadata"], false);
    assert_eq!(hidden_body["allowDownload"], false);
    let hidden_key = hidden_body["key"].as_str().unwrap();

    let stripped = call(
        &app,
        request(
            "GET",
            &format!("/api/shared-links/me?key={hidden_key}"),
            None,
            None,
        ),
    )
    .await;
    let stripped_body = json_body(&stripped);
    assert_eq!(stripped.status, 200, "{stripped_body}");
    assert_eq!(stripped_body["assets"][0]["hasMetadata"], false);
    assert!(stripped_body["assets"][0].get("checksum").is_none());
    assert_eq!(stripped_body["assets"][0]["width"], 16);

    let blocked = call(
        &app,
        request(
            "GET",
            &format!("/api/assets/{asset_id}/original?key={hidden_key}"),
            None,
            None,
        ),
    )
    .await;
    let blocked_body = json_body(&blocked);
    assert_eq!(blocked.status, 400, "{blocked_body}");
    assert_eq!(
        blocked_body["message"],
        "Not found or no asset.download access"
    );

    let still_visible = call(
        &app,
        request(
            "GET",
            &format!("/api/assets/{asset_id}/thumbnail?size=preview&key={hidden_key}"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(still_visible.status, 200);
    assert_eq!(&still_visible.body[..2], b"\xff\xd8");

    let removed = call(
        &app,
        request(
            "DELETE",
            &format!("/api/shared-links/{link_id}/assets"),
            Some(&session),
            Some(&format!(r#"{{"assetIds":["{asset_id}"]}}"#)),
        ),
    )
    .await;
    let removed_body = json_body(&removed);
    assert_eq!(removed.status, 200, "{removed_body}");
    assert_eq!(removed_body[0]["assetId"], asset_id);
    assert_eq!(removed_body[0]["success"], true);

    let deleted = call(
        &app,
        request(
            "DELETE",
            &format!("/api/shared-links/{link_id}"),
            Some(&session),
            None,
        ),
    )
    .await;
    assert_eq!(deleted.status, 204);
    assert!(deleted.body.is_empty());

    let gone = call(
        &app,
        request(
            "GET",
            &format!("/api/shared-links/{link_id}"),
            Some(&session),
            None,
        ),
    )
    .await;
    let gone_body = json_body(&gone);
    assert_eq!(gone.status, 400, "{gone_body}");
    assert_eq!(gone_body["message"], "Shared link not found");

    let _ = std::fs::remove_dir_all(&dir);
}
