use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
use futures_util::{SinkExt, StreamExt};
use http::Request;
use http_body_util::BodyExt;
use image::{ImageFormat, Rgb, RgbImage};
use immich_server::{App, Config};
use serde_json::Value;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
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

fn jpeg(width: u32, height: u32, red: u8) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        Rgb([red, (x % 251) as u8, (y % 251) as u8])
    });
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn upload_named(cookie: &str, bytes: &[u8], filename: &str) -> Request<Body> {
    let boundary = "immichboundary";
    let mut body = Vec::new();
    for (name, value) in [
        ("deviceAssetId", "device-1"),
        ("deviceId", "phone"),
        ("fileCreatedAt", "2020-01-02T03:04:05.000Z"),
        ("fileModifiedAt", "2020-01-02T03:04:05.000Z"),
        ("isFavorite", "false"),
        ("filename", filename),
    ] {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .into_bytes(),
        );
    }
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"{filename}\"\r\nContent-Type: image/jpeg\r\n\r\n"
        )
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

async fn add_user(app: &App, cookie: &str, email: &str, name: &str) -> (String, String) {
    let body = format!(r#"{{"email":"{email}","password":"partner-secret","name":"{name}"}}"#);
    let user = expect(
        app,
        json_request("POST", "/api/admin/users", Some(cookie), Some(&body)),
        201,
    )
    .await;
    let id = user["id"].as_str().unwrap().to_string();
    let (cookie, user_id) = sign_in(app, email, "partner-secret").await;
    assert_eq!(user_id, id);
    (cookie, id)
}

async fn expect(app: &App, request: Request<Body>, status: u16) -> Value {
    let (got, bytes) = call(app, request).await;
    let body = json_body(&bytes);
    assert_eq!(got, status, "{body}");
    body
}

async fn expect_empty(app: &App, request: Request<Body>, status: u16) {
    let (got, bytes) = call(app, request).await;
    assert_eq!(got, status, "{}", String::from_utf8_lossy(&bytes));
    assert!(bytes.is_empty(), "{}", String::from_utf8_lossy(&bytes));
}

async fn jpeg_size(app: &App, cookie: &str, asset_id: &str, edited: bool) -> (u32, u32) {
    let path = if edited {
        format!("/api/assets/{asset_id}/thumbnail?edited=true")
    } else {
        format!("/api/assets/{asset_id}/thumbnail")
    };
    let (status, bytes) = call(app, json_request("GET", &path, Some(cookie), None)).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let image = image::load_from_memory(&bytes).expect("jpeg");
    (image.width(), image.height())
}

fn edits_body(items: &str) -> String {
    format!(r#"{{"edits":[{items}]}}"#)
}

fn crop(x: i64, y: i64, width: i64, height: i64) -> String {
    format!(
        r#"{{"action":"crop","parameters":{{"x":{x},"y":{y},"width":{width},"height":{height}}}}}"#
    )
}

async fn sync_edits(app: &App, cookie: &str) -> Vec<Value> {
    let (status, bytes) = call(
        app,
        json_request(
            "POST",
            "/api/sync/stream",
            Some(cookie),
            Some(r#"{"types":["AssetEditsV1"]}"#),
        ),
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

fn of_type<'a>(lines: &'a [Value], kind: &str) -> Vec<&'a Value> {
    lines.iter().filter(|line| line["type"] == kind).collect()
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
async fn signed_in_asset_edits() {
    let dir = std::env::temp_dir().join(format!("immich-edits-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let (ada, _ada_id) = sign_in(&app, "admin@example.com", "correct horse").await;
    let (bea, bea_id) = add_user(&app, &ada, "bea@example.com", "Bea").await;
    let (cam, _cam_id) = add_user(&app, &ada, "cam@example.com", "Cam").await;

    let asset_id = expect(
        &app,
        upload_named(&ada, &jpeg(40, 20, 20), "cat.jpg"),
        201,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    sqlx::query("INSERT INTO asset_clip (assetId, embedding) VALUES (?, ?)")
        .bind(&asset_id)
        .bind([1u8, 2, 3, 4].as_slice())
        .execute(app.pool())
        .await
        .unwrap();

    let edits = format!("/api/assets/{asset_id}/edits");
    let denied = expect(&app, json_request("PUT", &edits, None, Some(r#"{"edits":[]}"#)), 401).await;
    assert_eq!(denied["message"], "Authentication required");
    let shared = expect(
        &app,
        json_request(
            "PUT",
            &edits,
            Some("immich_shared_link_token=abc"),
            Some(r#"{"edits":[]}"#),
        ),
        403,
    )
    .await;
    assert_eq!(shared["message"], "Forbidden");
    let stranger = expect(
        &app,
        json_request("PUT", &edits, Some(&cam), Some(&edits_body(&crop(0, 0, 10, 10)))),
        400,
    )
    .await;
    assert_eq!(
        stranger["message"],
        "Not found or no asset.edit.create access"
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = immich_server::router(app.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, server).await;
    });
    let mut ada_socket = connect_socket(addr, &ada).await;
    let mut cam_socket = connect_socket(addr, &cam).await;

    let saved = expect(
        &app,
        json_request(
            "PUT",
            &edits,
            Some(&ada),
            Some(&edits_body(&crop(0, 0, 10, 10))),
        ),
        200,
    )
    .await;
    assert_eq!(saved["assetId"], asset_id);
    assert_eq!(saved["edits"].as_array().unwrap().len(), 1);
    assert_eq!(saved["edits"][0]["action"], "crop");
    assert_eq!(saved["edits"][0]["parameters"]["width"], 10);
    assert!(saved["edits"][0]["parameters"].is_object());
    let crop_id = saved["edits"][0]["id"].as_str().unwrap().to_string();
    let again = expect(&app, json_request("GET", &edits, Some(&ada), None), 200).await;
    assert_eq!(again, saved);

    let asset = expect(
        &app,
        json_request("GET", &format!("/api/assets/{asset_id}"), Some(&ada), None),
        200,
    )
    .await;
    assert_eq!(asset["isEdited"], true);
    assert_eq!(asset["edits"].as_array().unwrap().len(), 1);
    assert_eq!(asset["width"], 10);
    assert_eq!(asset["height"], 10);
    assert_eq!(jpeg_size(&app, &ada, &asset_id, true).await, (10, 10));
    assert_eq!(jpeg_size(&app, &ada, &asset_id, false).await, (250, 125));

    let event = next_text(&mut ada_socket).await;
    assert!(event.starts_with("42"), "{event}");
    let payload: Value = serde_json::from_str(&event[2..]).unwrap();
    assert_eq!(payload[0], "AssetEditReadyV2");
    assert_eq!(payload[1]["asset"]["id"], asset_id);
    assert_eq!(payload[1]["asset"]["width"], 10);
    assert_eq!(payload[1]["asset"]["isEdited"], true);
    assert_eq!(payload[1]["edit"][0]["sequence"], 0);
    assert_eq!(payload[1]["edit"][0]["assetId"], asset_id);
    let quiet = tokio::time::timeout(std::time::Duration::from_millis(400), cam_socket.next()).await;
    assert!(quiet.is_err(), "guest received {quiet:?}");

    let duplicate = expect(
        &app,
        json_request(
            "PUT",
            &edits,
            Some(&ada),
            Some(&edits_body(&format!("{},{}", crop(0, 0, 10, 10), crop(0, 0, 4, 4)))),
        ),
        400,
    )
    .await;
    assert_eq!(duplicate["message"], "Duplicate edit actions are not allowed");
    let order = expect(
        &app,
        json_request(
            "PUT",
            &edits,
            Some(&ada),
            Some(&edits_body(&format!(
                r#"{{"action":"rotate","parameters":{{"angle":90}}}},{}"#,
                crop(0, 0, 1, 1)
            ))),
        ),
        400,
    )
    .await;
    assert_eq!(order["message"], "Crop action must be the first edit action");
    let bounds = expect(
        &app,
        json_request(
            "PUT",
            &edits,
            Some(&ada),
            Some(&edits_body(&crop(0, 0, 41, 10))),
        ),
        400,
    )
    .await;
    assert_eq!(bounds["message"], "Crop parameters are out of bounds");
    let angle = expect(
        &app,
        json_request(
            "PUT",
            &edits,
            Some(&ada),
            Some(&edits_body(
                r#"{"action":"rotate","parameters":{"angle":45}}"#,
            )),
        ),
        400,
    )
    .await;
    assert_eq!(
        angle["message"],
        "Angle must be one of the following values: 0, 90, 180, 270"
    );
    let kept = expect(&app, json_request("GET", &edits, Some(&ada), None), 200).await;
    assert_eq!(kept["edits"][0]["id"], crop_id);

    let first = sync_edits(&app, &ada).await;
    let upserts = of_type(&first, "AssetEditV1");
    assert_eq!(upserts.len(), 1);
    assert_eq!(upserts[0]["data"]["sequence"], 0);
    assert_eq!(upserts[0]["data"]["action"], "crop");
    let second = sync_edits(&app, &ada).await;
    assert!(of_type(&second, "AssetEditV1")
        .iter()
        .any(|line| line["data"]["id"] == crop_id));

    expect(
        &app,
        json_request(
            "POST",
            "/api/partners",
            Some(&ada),
            Some(&format!(r#"{{"sharedWithId":"{bea_id}"}}"#)),
        ),
        201,
    )
    .await;
    let partner = expect(&app, json_request("GET", &edits, Some(&bea), None), 400).await;
    assert_eq!(partner["message"], "Not found or no asset.read access");
    let album = expect(
        &app,
        json_request(
            "POST",
            "/api/albums",
            Some(&ada),
            Some(r#"{"albumName":"Edits"}"#),
        ),
        201,
    )
    .await;
    let album_id = album["id"].as_str().unwrap();
    expect(
        &app,
        json_request(
            "PUT",
            &format!("/api/albums/{album_id}/assets"),
            Some(&ada),
            Some(&format!(r#"{{"ids":["{asset_id}"]}}"#)),
        ),
        200,
    )
    .await;
    expect(
        &app,
        json_request(
            "PUT",
            &format!("/api/albums/{album_id}/users"),
            Some(&ada),
            Some(&format!(r#"{{"albumUsers":[{{"userId":"{bea_id}"}}]}}"#)),
        ),
        200,
    )
    .await;
    let member = expect(&app, json_request("GET", &edits, Some(&bea), None), 200).await;
    assert_eq!(member["edits"][0]["id"], crop_id);
    let member_put = expect(
        &app,
        json_request("PUT", &edits, Some(&bea), Some(&edits_body(&crop(0, 0, 8, 8)))),
        400,
    )
    .await;
    assert_eq!(
        member_put["message"],
        "Not found or no asset.edit.create access"
    );

    let rotated = expect(
        &app,
        json_request(
            "PUT",
            &edits,
            Some(&ada),
            Some(&edits_body(
                r#"{"action":"rotate","parameters":{"angle":90}}"#,
            )),
        ),
        200,
    )
    .await;
    assert_eq!(rotated["edits"][0]["action"], "rotate");
    let turned = expect(
        &app,
        json_request("GET", &format!("/api/assets/{asset_id}"), Some(&ada), None),
        200,
    )
    .await;
    assert_eq!(turned["isEdited"], true);
    assert_eq!(turned["width"], 20);
    assert_eq!(turned["height"], 40);
    let replaced = sync_edits(&app, &ada).await;
    assert!(of_type(&replaced, "AssetEditDeleteV1")
        .iter()
        .any(|line| line["data"]["editId"] == crop_id));
    assert!(of_type(&replaced, "AssetEditV1")
        .iter()
        .any(|line| line["data"]["action"] == "rotate"));

    expect_empty(&app, json_request("DELETE", &edits, Some(&ada), None), 204).await;
    let cleared = expect(&app, json_request("GET", &edits, Some(&ada), None), 200).await;
    assert_eq!(cleared["edits"].as_array().unwrap().len(), 0);
    let restored = expect(
        &app,
        json_request("GET", &format!("/api/assets/{asset_id}"), Some(&ada), None),
        200,
    )
    .await;
    assert_eq!(restored["isEdited"], false);
    assert_eq!(restored["width"], 40);
    assert_eq!(restored["height"], 20);
    assert_eq!(jpeg_size(&app, &ada, &asset_id, true).await, (250, 125));
    expect_empty(&app, json_request("DELETE", &edits, Some(&ada), None), 204).await;

    let person = expect(
        &app,
        json_request("POST", "/api/people", Some(&ada), Some("{}")),
        201,
    )
    .await;
    let person_id = person["id"].as_str().unwrap();
    let face_body = format!(
        r#"{{"assetId":"{asset_id}","personId":"{person_id}","imageWidth":40,"imageHeight":20,"x":1,"y":2,"width":3,"height":4}}"#
    );
    let unsigned = expect(
        &app,
        json_request("POST", "/api/faces", None, Some(&face_body)),
        401,
    )
    .await;
    assert_eq!(unsigned["message"], "Authentication required");
    expect(
        &app,
        json_request(
            "POST",
            "/api/faces",
            Some("immich_shared_link_token=abc"),
            Some(&face_body),
        ),
        403,
    )
    .await;
    let missing_asset = expect(
        &app,
        json_request(
            "POST",
            "/api/faces",
            Some(&ada),
            Some(
                r#"{"assetId":"00000000-0000-4000-8000-000000000099","personId":"00000000-0000-4000-8000-0000000000aa","imageWidth":10,"imageHeight":10,"x":0,"y":0,"width":1,"height":1}"#,
            ),
        ),
        404,
    )
    .await;
    assert_eq!(missing_asset["message"], "Asset not found");
    let missing_person = expect(
        &app,
        json_request(
            "POST",
            "/api/faces",
            Some(&ada),
            Some(&format!(
                r#"{{"assetId":"{asset_id}","personId":"00000000-0000-4000-8000-0000000000aa","imageWidth":40,"imageHeight":20,"x":0,"y":0,"width":1,"height":1}}"#
            )),
        ),
        400,
    )
    .await;
    assert_eq!(missing_person["message"], "Person not found");
    let unowned = expect(
        &app,
        json_request("POST", "/api/faces", Some(&cam), Some(&face_body)),
        400,
    )
    .await;
    assert_eq!(unowned["message"], "Not found or no asset.update access");
    let bea_person = expect(
        &app,
        json_request("POST", "/api/people", Some(&bea), Some("{}")),
        201,
    )
    .await;
    let foreign = expect(
        &app,
        json_request(
            "POST",
            "/api/faces",
            Some(&ada),
            Some(&format!(
                r#"{{"assetId":"{asset_id}","personId":"{}","imageWidth":40,"imageHeight":20,"x":0,"y":0,"width":1,"height":1}}"#,
                bea_person["id"].as_str().unwrap()
            )),
        ),
        400,
    )
    .await;
    assert_eq!(foreign["message"], "Not found or no person.read access");
    expect_empty(
        &app,
        json_request("POST", "/api/faces", Some(&ada), Some(&face_body)),
        201,
    )
    .await;
    let faces = expect(
        &app,
        json_request(
            "GET",
            &format!("/api/faces?id={asset_id}"),
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    let manual = faces
        .as_array()
        .unwrap()
        .iter()
        .find(|face| face["boundingBoxX1"] == 1)
        .unwrap();
    assert_eq!(manual["sourceType"], "manual");
    assert_eq!(manual["boundingBoxY1"], 2);
    assert_eq!(manual["boundingBoxX2"], 4);
    assert_eq!(manual["boundingBoxY2"], 6);
    assert_eq!(manual["imageWidth"], 40);
    assert_eq!(manual["imageHeight"], 20);
    let thumb = manual["person"]["thumbnailPath"].as_str().unwrap();
    assert!(Path::new(thumb).is_file(), "{thumb}");

    expect(
        &app,
        json_request(
            "PUT",
            &edits,
            Some(&ada),
            Some(&edits_body(&crop(10, 0, 10, 10))),
        ),
        200,
    )
    .await;
    expect_empty(
        &app,
        json_request(
            "POST",
            "/api/faces",
            Some(&ada),
            Some(&format!(
                r#"{{"assetId":"{asset_id}","personId":"{person_id}","imageWidth":10,"imageHeight":10,"x":0,"y":0,"width":4,"height":4}}"#
            )),
        ),
        201,
    )
    .await;
    let faces = expect(
        &app,
        json_request(
            "GET",
            &format!("/api/faces?id={asset_id}"),
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    let inverse = faces
        .as_array()
        .unwrap()
        .iter()
        .find(|face| face["boundingBoxX1"] == 10)
        .expect("inverse box");
    assert_eq!(inverse["boundingBoxY1"], 0);
    assert_eq!(inverse["boundingBoxX2"], 14);
    assert_eq!(inverse["boundingBoxY2"], 4);
    assert_eq!(inverse["imageWidth"], 40);
    assert_eq!(inverse["imageHeight"], 20);
    assert_eq!(inverse["sourceType"], "manual");

    let mirrors = expect(
        &app,
        json_request(
            "PUT",
            &edits,
            Some(&ada),
            Some(&edits_body(
                r#"{"action":"mirror","parameters":{"axis":"horizontal"}},{"action":"mirror","parameters":{"axis":"vertical"}}"#,
            )),
        ),
        200,
    )
    .await;
    assert_eq!(mirrors["edits"].as_array().unwrap().len(), 2);
    let mirrored = expect(
        &app,
        json_request("GET", &format!("/api/assets/{asset_id}"), Some(&ada), None),
        200,
    )
    .await;
    assert_eq!(mirrored["width"], 40);
    assert_eq!(mirrored["height"], 20);
    let same_axis = expect(
        &app,
        json_request(
            "PUT",
            &edits,
            Some(&ada),
            Some(&edits_body(
                r#"{"action":"mirror","parameters":{"axis":"horizontal"}},{"action":"mirror","parameters":{"axis":"horizontal"}}"#,
            )),
        ),
        400,
    )
    .await;
    assert_eq!(same_axis["message"], "Duplicate edit actions are not allowed");
    let still = expect(&app, json_request("GET", &edits, Some(&ada), None), 200).await;
    assert_eq!(still["edits"].as_array().unwrap().len(), 2);

    let gif_id = expect(
        &app,
        upload_named(&ada, &jpeg(18, 12, 7), "cat.gif"),
        201,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let gif = expect(
        &app,
        json_request(
            "PUT",
            &format!("/api/assets/{gif_id}/edits"),
            Some(&ada),
            Some(&edits_body(&crop(0, 0, 2, 2))),
        ),
        400,
    )
    .await;
    assert_eq!(gif["message"], "Editing GIF images is not supported");

    let locked_id = expect(
        &app,
        upload_named(&ada, &jpeg(22, 14, 8), "locked.jpg"),
        201,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    expect(
        &app,
        json_request(
            "PUT",
            &format!("/api/assets/{locked_id}"),
            Some(&ada),
            Some(r#"{"visibility":"locked"}"#),
        ),
        200,
    )
    .await;
    let locked = expect(
        &app,
        json_request(
            "PUT",
            &format!("/api/assets/{locked_id}/edits"),
            Some(&ada),
            Some(&edits_body(&crop(0, 0, 2, 2))),
        ),
        400,
    )
    .await;
    assert_eq!(locked["message"], "Not found or no asset.edit.create access");

    sqlx::query(
        "INSERT INTO asset_edit_audit (id, editId, assetId, deletedAt) VALUES (?, ?, ?, ?), (?, ?, ?, ?)",
    )
    .bind("old-edit-audit")
    .bind("edit-old")
    .bind(&asset_id)
    .bind("2000-01-01T00:00:00.000Z")
    .bind("new-edit-audit")
    .bind("edit-new")
    .bind(&asset_id)
    .bind("2099-01-01T00:00:00.000Z")
    .execute(app.pool())
    .await
    .unwrap();
    app.prune_edit_audits().await.unwrap();
    let old: Option<String> = sqlx::query_scalar("SELECT id FROM asset_edit_audit WHERE id = ?")
        .bind("old-edit-audit")
        .fetch_optional(app.pool())
        .await
        .unwrap();
    let fresh: Option<String> = sqlx::query_scalar("SELECT id FROM asset_edit_audit WHERE id = ?")
        .bind("new-edit-audit")
        .fetch_optional(app.pool())
        .await
        .unwrap();
    assert!(old.is_none());
    assert_eq!(fresh.as_deref(), Some("new-edit-audit"));

    let embedding: Vec<u8> = sqlx::query_scalar("SELECT embedding FROM asset_clip WHERE assetId = ?")
        .bind(&asset_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(embedding, vec![1, 2, 3, 4]);
}
