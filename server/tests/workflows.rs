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
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| panic!("body {}", String::from_utf8_lossy(bytes)))
}

fn request(method: &str, path: &str, cookie: Option<&str>, body: Option<&str>) -> Request<Body> {
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

fn upload(cookie: &str, bytes: &[u8], device_asset_id: &str) -> Request<Body> {
    let boundary = "immichboundary";
    let mut body = Vec::new();
    for (name, value) in [
        ("deviceAssetId", device_asset_id),
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
        .oneshot(request(
            "POST",
            "/api/auth/login",
            None,
            Some(r#"{"email":"admin@example.com","password":"correct horse"}"#),
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

fn message(bytes: &[u8]) -> String {
    json_body(bytes)["message"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn editor_manifest() -> String {
    json!({
        "name": "pi-test",
        "version": "1.0.0",
        "title": "Pi test",
        "description": "Editor plugin",
        "author": "Ada",
        "wasmPath": "plugin.wasm",
        "methods": [
            {
                "name": "noopone",
                "title": "Noop",
                "description": "Skip",
                "types": ["AssetV1"]
            },
            {
                "name": "odd",
                "title": "Odd",
                "description": "Incompatible after the type is replaced",
                "types": ["AssetV1"]
            }
        ],
        "templates": [{
            "name": "mark",
            "title": "Mark",
            "description": "Favorite new assets",
            "trigger": "AssetCreate",
            "steps": [{"method": "pi-test#noopone"}]
        }]
    })
    .to_string()
}

fn write_plugin(folder: &Path, wasm: &[u8], manifest: &str) {
    std::fs::create_dir_all(folder).unwrap();
    std::fs::write(folder.join("plugin.wasm"), wasm).unwrap();
    std::fs::write(folder.join("manifest.json"), manifest).unwrap();
}

async fn prepare(name: &str) -> (App, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("immich-workflow-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    (app, dir)
}

#[tokio::test]
async fn workflow_editor_matches_the_saved_plugin() {
    let (app, dir) = prepare("editor").await;
    let cookie = sign_in(&app).await;

    let (status, bytes) = call(&app, request("GET", "/api/workflows", None, None)).await;
    assert_eq!(status, 401, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Authentication required");
    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/workflows",
            None,
            Some(r#"{"trigger":"AssetCreate"}"#),
        ),
    )
    .await;
    assert_eq!(status, 401, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Authentication required");
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/plugins/methods",
            Some("immich_shared_link_token=link"),
            None,
        ),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Forbidden");
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/workflows",
            Some("immich_shared_link_token=link"),
            None,
        ),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Forbidden");

    let (status, bytes) = call(
        &app,
        request("GET", "/api/workflows/triggers", Some(&cookie), None),
    )
    .await;
    let triggers = json_body(&bytes);
    assert_eq!(status, 200, "{triggers}");
    assert_eq!(
        triggers,
        json!([
            {"trigger": "AssetCreate", "types": ["AssetV1"]},
            {"trigger": "AssetMetadataExtraction", "types": ["AssetV1"]},
            {"trigger": "AssetTagged", "types": ["AssetV1"]}
        ])
    );
    let (status, bytes) = call(
        &app,
        request("GET", "/api/plugins/methods", Some(&cookie), None),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(json_body(&bytes), json!([]));
    let (status, bytes) = call(
        &app,
        request("GET", "/api/plugins/templates", Some(&cookie), None),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(json_body(&bytes), json!([]));
    let (status, bytes) = call(&app, request("GET", "/api/plugins", Some(&cookie), None)).await;
    assert_eq!(json_body(&bytes), json!([]));
    assert_eq!(status, 200);

    let bad = dir.join("bad-plugin");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join("manifest.json"), "{").unwrap();
    app.import_plugin_folder(&bad, true).await;
    let missing = dir.join("missing-wasm");
    std::fs::create_dir_all(&missing).unwrap();
    std::fs::write(
        missing.join("manifest.json"),
        r#"{"name":"missing-wasm","version":"1.0.0","title":"Missing","description":"No wasm","author":"Ada","wasmPath":"nope.wasm"}"#,
    )
    .unwrap();
    app.import_plugin_folder(&missing, true).await;
    let (status, bytes) = call(&app, request("GET", "/api/plugins", Some(&cookie), None)).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(json_body(&bytes), json!([]));

    let plugin_dir = dir.join("pi-test");
    let wasm = wat::parse_str("(module)").unwrap();
    write_plugin(&plugin_dir, &wasm, &editor_manifest());
    app.import_plugin_folder(&plugin_dir, false).await;
    let (status, bytes) = call(&app, request("GET", "/api/plugins", Some(&cookie), None)).await;
    let plugins = json_body(&bytes);
    assert_eq!(status, 200, "{plugins}");
    assert_eq!(plugins.as_array().unwrap().len(), 1);
    assert_eq!(plugins[0]["name"], "pi-test");
    assert_eq!(plugins[0]["title"], "Pi test");
    let plugin_id = plugins[0]["id"].as_str().unwrap().to_string();

    sqlx::query("UPDATE plugin SET title = 'Changed' WHERE name = 'pi-test'")
        .execute(app.pool())
        .await
        .unwrap();
    app.import_plugin_folder(&plugin_dir, false).await;
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/plugins/{plugin_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let skipped = json_body(&bytes);
    assert_eq!(status, 200, "{skipped}");
    assert_eq!(skipped["title"], "Changed");
    app.import_plugin_folder(&plugin_dir, true).await;
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/plugins/{plugin_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let forced = json_body(&bytes);
    assert_eq!(status, 200, "{forced}");
    assert_eq!(forced["title"], "Pi test");
    assert_eq!(forced["methods"].as_array().unwrap().len(), 2);

    let (status, bytes) = call(
        &app,
        request("GET", "/api/plugins/not-a-uuid", Some(&cookie), None),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Validation failed");
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/plugins/00000000-0000-4000-8000-000000000001",
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Plugin not found");

    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/plugins/methods?trigger=AssetCreate",
            Some(&cookie),
            None,
        ),
    )
    .await;
    let methods = json_body(&bytes);
    assert_eq!(status, 200, "{methods}");
    assert_eq!(methods.as_array().unwrap().len(), 2);
    assert_eq!(methods[0]["key"], "pi-test#noopone");
    assert_eq!(methods[0]["hostFunctions"], false);
    assert!(methods[0].get("schema").is_none());
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/plugins/methods?trigger=Nope",
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(json_body(&bytes), json!([]));
    assert_eq!(status, 200);
    let (status, bytes) = call(
        &app,
        request("GET", "/api/plugins/templates", Some(&cookie), None),
    )
    .await;
    let templates = json_body(&bytes);
    assert_eq!(status, 200, "{templates}");
    assert_eq!(templates[0]["key"], "pi-test#mark");
    assert_eq!(templates[0]["trigger"], "AssetCreate");
    assert_eq!(templates[0]["steps"][0]["method"], "pi-test#noopone");
    assert!(templates[0]["steps"][0]["config"].is_null());
    assert!(templates[0]["steps"][0].get("enabled").is_none());

    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/workflows",
            Some(&cookie),
            Some(r#"{"trigger":"Nope"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");
    let (status, bytes) = call(
        &app,
        request("POST", "/api/workflows", Some(&cookie), Some("[1]")),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");
    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/workflows",
            Some(&cookie),
            Some(r#"{"trigger":"AssetCreate","steps":[{"method":"missing#nope"}]}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Unknown method missing#nope");

    sqlx::query(r#"UPDATE plugin_method SET types = '["Other"]' WHERE name = 'odd'"#)
        .execute(app.pool())
        .await
        .unwrap();
    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/workflows",
            Some(&cookie),
            Some(r#"{"trigger":"AssetCreate","steps":[{"method":"pi-test#odd"}]}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        message(&bytes),
        "Method \"pi-test#odd\" is incompatible with workflow trigger: \"AssetCreate\""
    );

    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/workflows",
            Some(&cookie),
            Some(
                r#"{"trigger":"AssetCreate","name":"Mark","logging":true,"steps":[{"method":"pi-test#noopone","config":{"note":"a"}}]}"#,
            ),
        ),
    )
    .await;
    let created = json_body(&bytes);
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["steps"], json!([]));
    assert_eq!(created["logging"], true);
    assert_eq!(created["enabled"], true);
    assert_eq!(created["name"], "Mark");
    let workflow_id = created["id"].as_str().unwrap().to_string();

    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{workflow_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let stored = json_body(&bytes);
    assert_eq!(status, 200, "{stored}");
    assert_eq!(stored["steps"][0]["method"], "pi-test#noopone");
    assert_eq!(stored["steps"][0]["config"], json!({"note": "a"}));
    assert_eq!(stored["steps"][0]["enabled"], true);

    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{workflow_id}/share"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let share = json_body(&bytes);
    assert_eq!(status, 200, "{share}");
    assert!(share.get("id").is_none());
    assert!(share.get("createdAt").is_none());
    assert!(share.get("updatedAt").is_none());
    assert!(share.get("enabled").is_none());
    assert!(share.get("logging").is_none());
    assert_eq!(share["trigger"], "AssetCreate");
    assert_eq!(share["name"], "Mark");
    assert!(share["steps"][0].get("enabled").is_none());

    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/workflows",
            Some(&cookie),
            Some(r#"{"trigger":"AssetTagged","steps":[{"method":"pi-test#noopone","enabled":false}]}"#),
        ),
    )
    .await;
    let quiet = json_body(&bytes);
    assert_eq!(status, 201, "{quiet}");
    let quiet_id = quiet["id"].as_str().unwrap();
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{quiet_id}/share"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let quiet_share = json_body(&bytes);
    assert_eq!(status, 200, "{quiet_share}");
    assert_eq!(quiet_share["steps"][0]["enabled"], false);

    let (status, bytes) = call(
        &app,
        request("GET", "/api/workflows/not-a-uuid", Some(&cookie), None),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/workflows/00000000-0000-4000-8000-000000000099",
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Not found or no workflow.read access");
    let (status, bytes) = call(
        &app,
        request(
            "DELETE",
            "/api/workflows/00000000-0000-4000-8000-000000000099",
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(message(&bytes), "Not found or no workflow.delete access");
    assert_eq!(status, 400);

    let (status, bytes) = call(
        &app,
        request("GET", "/api/workflows?enabled=maybe", Some(&cookie), None),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");
    let (status, bytes) = call(
        &app,
        request("GET", "/api/workflows?enabled=true", Some(&cookie), None),
    )
    .await;
    let listed = json_body(&bytes);
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed.as_array().unwrap().len(), 2);
    assert_eq!(listed[0]["id"], quiet_id);

    let (status, bytes) = call(&app, upload(&cookie, &jpeg(1), "editor-1")).await;
    let uploaded = json_body(&bytes);
    assert_eq!(status, 201, "{uploaded}");
    let asset_id = uploaded["id"].as_str().unwrap();
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{workflow_id}/logs"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let logs = json_body(&bytes);
    assert_eq!(status, 200, "{logs}");
    assert_eq!(logs.as_array().unwrap().len(), 1);
    assert_eq!(logs[0]["result"], "completed");
    assert_eq!(logs[0]["triggerDataId"], asset_id);
    assert!(logs[0].get("lastStep").is_none());
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{workflow_id}/logs?result=nope"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{workflow_id}/logs?limit=0"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{workflow_id}/logs?before=yesterday"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{workflow_id}/logs?before=2000-01-01T00:00:00Z"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(json_body(&bytes), json!([]));

    let (status, bytes) = call(
        &app,
        request(
            "PUT",
            &format!("/api/workflows/{workflow_id}"),
            Some(&cookie),
            Some(r#"{"logging":false,"name":"Quiet"}"#),
        ),
    )
    .await;
    let updated = json_body(&bytes);
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["logging"], false);
    assert_eq!(updated["name"], "Quiet");
    assert_eq!(updated["steps"][0]["method"], "pi-test#noopone");
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{workflow_id}/logs"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(json_body(&bytes), json!([]));

    let (status, bytes) = call(&app, upload(&cookie, &jpeg(2), "editor-2")).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{workflow_id}/logs"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(json_body(&bytes), json!([]));
    assert_eq!(status, 200);
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{quiet_id}/logs"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(json_body(&bytes), json!([]));
    assert_eq!(status, 200);

    let (status, bytes) = call(
        &app,
        request(
            "PATCH",
            &format!("/api/workflows/{workflow_id}"),
            Some(&cookie),
            Some(r#"{"steps":[]}"#),
        ),
    )
    .await;
    let patched = json_body(&bytes);
    assert_eq!(status, 200, "{patched}");
    assert_eq!(patched["steps"], json!([]));
    let (status, bytes) = call(
        &app,
        request(
            "DELETE",
            &format!("/api/workflows/{workflow_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/workflows/{workflow_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Not found or no workflow.read access");

    let _ = std::fs::remove_dir_all(&dir);
}

fn emit(name: &str, payload: &str) -> String {
    let mut stores = String::new();
    for (index, byte) in payload.bytes().enumerate() {
        stores.push_str(&format!(
            "(call $store_u8 (i64.add (local.get $out) (i64.const {index})) (i32.const {byte}))\n"
        ));
    }
    format!(
        r#"(func (export "{name}") (result i32)
            (local $out i64)
            (local.set $out (call $alloc (i64.const {len})))
            {stores}
            (call $output_set (local.get $out) (i64.const {len}))
            (i32.const 0))"#,
        len = payload.len()
    )
}

fn host_emit(name: &str, payload: &str) -> String {
    let mut stores = String::new();
    for (index, byte) in payload.bytes().enumerate() {
        stores.push_str(&format!(
            "(call $store_u8 (i64.add (local.get $out) (i64.const {index})) (i32.const {byte}))\n"
        ));
    }
    format!(
        r#"(func (export "{name}") (result i32)
            (local $out i64)
            (drop (call $bulkTagAssets (i64.const 0)))
            (local.set $out (call $alloc (i64.const {len})))
            {stores}
            (call $output_set (local.get $out) (i64.const {len}))
            (i32.const 0))"#,
        len = payload.len()
    )
}

fn runner_wasm() -> Vec<u8> {
    let source = format!(
        r#"(module
            (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
            (import "extism:host/env" "store_u8" (func $store_u8 (param i64 i32)))
            (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))
            (import "extism:host/user" "bulkTagAssets" (func $bulkTagAssets (param i64) (result i64)))
            {halt}
            {favorite}
            (func (export "boom") (result i32) unreachable)
            {ask}
            {askyes}
        )"#,
        halt = emit("halt", r#"{"workflow":{"continue":false}}"#),
        favorite = emit("favorite", r#"{"changes":{"asset":{"isFavorite":true}}}"#),
        ask = host_emit("ask", r#"{"workflow":{"continue":true}}"#),
        askyes = host_emit("askyes", r#"{"workflow":{"continue":true}}"#),
    );
    wat::parse_str(source).expect("wat")
}

fn runner_manifest() -> String {
    let method = |name: &str, host: bool| {
        json!({
            "name": name,
            "title": name,
            "description": name,
            "types": ["AssetV1"],
            "hostFunctions": host,
        })
    };
    json!({
        "name": "pi-test",
        "version": "1.0.0",
        "title": "Runner",
        "description": "Runs stored wasm",
        "author": "Ada",
        "wasmPath": "plugin.wasm",
        "methods": [
            method("halt", false),
            method("favorite", false),
            method("boom", false),
            method("ask", false),
            method("askyes", true),
            method("noopmeta", false),
            method("nooptag", false),
        ]
    })
    .to_string()
}

async fn post_workflow(app: &App, cookie: &str, body: &str) -> Value {
    let (status, bytes) = call(
        app,
        request("POST", "/api/workflows", Some(cookie), Some(body)),
    )
    .await;
    let created = json_body(&bytes);
    assert_eq!(status, 201, "{created}");
    created
}

async fn patch_workflow(app: &App, cookie: &str, id: &str, body: &str) {
    let (status, bytes) = call(
        app,
        request(
            "PATCH",
            &format!("/api/workflows/{id}"),
            Some(cookie),
            Some(body),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
}

async fn logs(app: &App, cookie: &str, id: &str) -> Value {
    let (status, bytes) = call(
        app,
        request(
            "GET",
            &format!("/api/workflows/{id}/logs"),
            Some(cookie),
            None,
        ),
    )
    .await;
    let logs = json_body(&bytes);
    assert_eq!(status, 200, "{logs}");
    logs
}

async fn favorite(app: &App, cookie: &str, asset_id: &str) -> bool {
    let (status, bytes) = call(
        app,
        request(
            "GET",
            &format!("/api/assets/{asset_id}"),
            Some(cookie),
            None,
        ),
    )
    .await;
    let asset = json_body(&bytes);
    assert_eq!(status, 200, "{asset}");
    asset["isFavorite"].as_bool().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn stored_wasm_runs_on_upload_and_tag() {
    let (app, dir) = prepare("run").await;
    let cookie = sign_in(&app).await;
    let plugin_dir = dir.join("pi-test");
    write_plugin(&plugin_dir, &runner_wasm(), &runner_manifest());
    app.import_plugin_folder(&plugin_dir, true).await;
    let (status, bytes) = call(&app, request("GET", "/api/plugins", Some(&cookie), None)).await;
    let plugins = json_body(&bytes);
    assert_eq!(status, 200, "{plugins}");
    let plugin_id = plugins[0]["id"].as_str().unwrap();

    let halted = post_workflow(
        &app,
        &cookie,
        r#"{"trigger":"AssetCreate","logging":true,"steps":[{"method":"pi-test#halt"}]}"#,
    )
    .await;
    let halted_id = halted["id"].as_str().unwrap();
    let (status, bytes) = call(&app, upload(&cookie, &jpeg(10), "run-halt")).await;
    let uploaded = json_body(&bytes);
    assert_eq!(status, 201, "{uploaded}");
    let asset_id = uploaded["id"].as_str().unwrap();
    assert!(!favorite(&app, &cookie, asset_id).await);
    let halted_logs = logs(&app, &cookie, halted_id).await;
    assert_eq!(halted_logs[0]["result"], "halted");
    assert_eq!(halted_logs[0]["lastStep"]["index"], 0);
    assert_eq!(
        halted_logs[0]["lastStep"]["method"],
        format!("{plugin_id}#halt")
    );
    patch_workflow(&app, &cookie, halted_id, r#"{"enabled":false}"#).await;

    let favored = post_workflow(
        &app,
        &cookie,
        r#"{"trigger":"AssetCreate","logging":true,"steps":[{"method":"pi-test#favorite"}]}"#,
    )
    .await;
    let favored_id = favored["id"].as_str().unwrap();
    let (status, bytes) = call(&app, upload(&cookie, &jpeg(11), "run-favorite")).await;
    let uploaded = json_body(&bytes);
    assert_eq!(status, 201, "{uploaded}");
    let asset_id = uploaded["id"].as_str().unwrap();
    assert!(favorite(&app, &cookie, asset_id).await);
    assert_eq!(
        logs(&app, &cookie, favored_id).await[0]["result"],
        "completed"
    );
    assert!(logs(&app, &cookie, favored_id).await[0]
        .get("lastStep")
        .is_none());
    patch_workflow(&app, &cookie, favored_id, r#"{"enabled":false}"#).await;

    let broken = post_workflow(
        &app,
        &cookie,
        r#"{"trigger":"AssetCreate","logging":true,"steps":[{"method":"pi-test#boom"}]}"#,
    )
    .await;
    let broken_id = broken["id"].as_str().unwrap();
    let (status, bytes) = call(&app, upload(&cookie, &jpeg(12), "run-boom")).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    let boom_logs = logs(&app, &cookie, broken_id).await;
    assert_eq!(boom_logs[0]["result"], "error");
    assert_eq!(
        boom_logs[0]["lastStep"]["method"],
        format!("{plugin_id}#boom")
    );
    patch_workflow(&app, &cookie, broken_id, r#"{"enabled":false}"#).await;

    let refused = post_workflow(
        &app,
        &cookie,
        r#"{"trigger":"AssetCreate","logging":true,"steps":[{"method":"pi-test#ask"}]}"#,
    )
    .await;
    let refused_id = refused["id"].as_str().unwrap();
    let (status, bytes) = call(&app, upload(&cookie, &jpeg(13), "run-ask")).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(logs(&app, &cookie, refused_id).await[0]["result"], "error");
    patch_workflow(&app, &cookie, refused_id, r#"{"enabled":false}"#).await;

    let allowed = post_workflow(
        &app,
        &cookie,
        r#"{"trigger":"AssetCreate","logging":true,"steps":[{"method":"pi-test#askyes"}]}"#,
    )
    .await;
    let allowed_id = allowed["id"].as_str().unwrap();
    let (status, bytes) = call(&app, upload(&cookie, &jpeg(14), "run-askyes")).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        logs(&app, &cookie, allowed_id).await[0]["result"],
        "completed"
    );
    patch_workflow(&app, &cookie, allowed_id, r#"{"enabled":false}"#).await;

    let stopped = post_workflow(
        &app,
        &cookie,
        r#"{"trigger":"AssetCreate","logging":true,"steps":[{"method":"pi-test#halt"},{"method":"pi-test#favorite"}]}"#,
    )
    .await;
    let stopped_id = stopped["id"].as_str().unwrap();
    let (status, bytes) = call(&app, upload(&cookie, &jpeg(15), "run-two")).await;
    let uploaded = json_body(&bytes);
    assert_eq!(status, 201, "{uploaded}");
    let asset_id = uploaded["id"].as_str().unwrap();
    assert!(!favorite(&app, &cookie, asset_id).await);
    assert_eq!(logs(&app, &cookie, stopped_id).await[0]["result"], "halted");
    patch_workflow(&app, &cookie, stopped_id, r#"{"enabled":false}"#).await;

    let skipped = post_workflow(
        &app,
        &cookie,
        r#"{"trigger":"AssetCreate","logging":true,"steps":[{"method":"pi-test#favorite","enabled":false},{"method":"pi-test#halt"}]}"#,
    )
    .await;
    let skipped_id = skipped["id"].as_str().unwrap();
    let (status, bytes) = call(&app, upload(&cookie, &jpeg(16), "run-skip")).await;
    let uploaded = json_body(&bytes);
    assert_eq!(status, 201, "{uploaded}");
    assert!(!favorite(&app, &cookie, uploaded["id"].as_str().unwrap()).await);
    let skipped_logs = logs(&app, &cookie, skipped_id).await;
    assert_eq!(skipped_logs[0]["result"], "halted");
    assert_eq!(skipped_logs[0]["lastStep"]["index"], 1);
    patch_workflow(&app, &cookie, skipped_id, r#"{"enabled":false}"#).await;

    let idle = post_workflow(
        &app,
        &cookie,
        r#"{"trigger":"AssetCreate","enabled":false,"logging":true,"steps":[{"method":"pi-test#favorite"}]}"#,
    )
    .await;
    let idle_id = idle["id"].as_str().unwrap();
    let created = post_workflow(
        &app,
        &cookie,
        r#"{"trigger":"AssetCreate","logging":true,"steps":[{"method":"pi-test#favorite"}]}"#,
    )
    .await;
    let created_id = created["id"].as_str().unwrap();
    let extracted = post_workflow(
        &app,
        &cookie,
        r#"{"trigger":"AssetMetadataExtraction","logging":true,"steps":[{"method":"pi-test#noopmeta"}]}"#,
    )
    .await;
    let extracted_id = extracted["id"].as_str().unwrap();
    let (status, bytes) = call(&app, upload(&cookie, &jpeg(17), "run-both")).await;
    let uploaded = json_body(&bytes);
    assert_eq!(status, 201, "{uploaded}");
    let asset_id = uploaded["id"].as_str().unwrap().to_string();
    assert!(favorite(&app, &cookie, &asset_id).await);
    assert_eq!(
        logs(&app, &cookie, created_id).await[0]["result"],
        "completed"
    );
    assert_eq!(
        logs(&app, &cookie, extracted_id)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(logs(&app, &cookie, idle_id).await, json!([]));

    let described = format!(r#"{{"ids":["{asset_id}"],"description":"hello"}}"#);
    let (status, bytes) = call(
        &app,
        request("PUT", "/api/assets", Some(&cookie), Some(&described)),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        logs(&app, &cookie, extracted_id)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        logs(&app, &cookie, created_id)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let tagged = post_workflow(
        &app,
        &cookie,
        r#"{"trigger":"AssetTagged","logging":true,"steps":[{"method":"pi-test#nooptag"}]}"#,
    )
    .await;
    let tagged_id = tagged["id"].as_str().unwrap();
    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/tags",
            Some(&cookie),
            Some(r#"{"name":"Trip"}"#),
        ),
    )
    .await;
    let tag = json_body(&bytes);
    assert_eq!(status, 201, "{tag}");
    let tag_id = tag["id"].as_str().unwrap();
    let (status, bytes) = call(
        &app,
        request(
            "PUT",
            &format!("/api/tags/{tag_id}/assets"),
            Some(&cookie),
            Some(&format!(r#"{{"ids":["{asset_id}"]}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        logs(&app, &cookie, tagged_id)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        logs(&app, &cookie, extracted_id)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let _ = std::fs::remove_dir_all(&dir);
}
