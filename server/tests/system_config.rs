use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
use immich_server::{App, Config};
use serde_json::{json, Value};
use tower::ServiceExt;

fn config(dir: &Path, config_file: Option<&Path>) -> Config {
    Config {
        data_dir: dir.to_path_buf(),
        host: IpAddr::from([127, 0, 0, 1]),
        port: 0,
        admin_email: Some("admin@example.com".to_string()),
        admin_name: Some("Ada".to_string()),
        admin_password: Some("correct horse".to_string()),
        s3: None,
        web_root: dir.join("web"),
        machine_learning_url: "http://ml.test:3003".to_string(),
        machine_learning_enabled: true,
        config_file: config_file.map(Path::to_path_buf),
        log_level: Some("log".to_string()),
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

fn access_cookie(headers: &http::HeaderMap) -> String {
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("immich_access_token="))
        .expect("access cookie")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_string()
}

async fn sign_in(app: &App) -> String {
    let reply = call(
        app,
        request(
            "POST",
            "/api/auth/login",
            None,
            Some(r#"{"email":"admin@example.com","password":"correct horse"}"#),
        ),
    )
    .await;
    assert_eq!(
        reply.status,
        201,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    access_cookie(&reply.headers)
}

#[tokio::test]
async fn admin_config_saves_the_difference_from_the_defaults() {
    let dir = std::env::temp_dir().join(format!("immich-config-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir, None)).await.expect("prepare");
    let session = sign_in(&app).await;

    let missing = call(&app, request("GET", "/api/system-config", None, None)).await;
    let missing_body = json_body(&missing);
    assert_eq!(missing.status, 401, "{missing_body}");
    assert_eq!(missing_body["message"], "Authentication required");

    let defaults = call(
        &app,
        request("GET", "/api/system-config/defaults", Some(&session), None),
    )
    .await;
    let defaults_body = json_body(&defaults);
    assert_eq!(defaults.status, 200, "{defaults_body}");
    assert_eq!(defaults_body["trash"]["days"], 30);
    assert_eq!(
        defaults_body["machineLearning"]["urls"][0],
        "http://ml.test:3003"
    );
    assert_eq!(
        defaults_body["machineLearning"]["clip"]["modelName"],
        "ViT-B-32__openai"
    );
    assert!(defaults_body["ffmpeg"]["realtime"]["enabled"].is_boolean());
    assert!(defaults_body.get("job").is_some());
    assert!(defaults_body.get("notifications").is_some());

    let current = call(
        &app,
        request("GET", "/api/system-config", Some(&session), None),
    )
    .await;
    let mut document = json_body(&current);
    assert_eq!(current.status, 200, "{document}");
    assert_eq!(document["trash"]["days"], 30);

    let admin_defaults = call(
        &app,
        request("GET", "/api/admin/config/defaults", Some(&session), None),
    )
    .await;
    assert_eq!(admin_defaults.status, 200);
    assert_eq!(json_body(&admin_defaults)["trash"]["days"], 30);

    let options = call(
        &app,
        request(
            "GET",
            "/api/system-config/storage-template-options",
            Some(&session),
            None,
        ),
    )
    .await;
    let options_body = json_body(&options);
    assert_eq!(options.status, 200, "{options_body}");
    assert_eq!(options_body["yearOptions"][0], "y");
    assert!(options_body["presetOptions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|preset| preset.as_str().unwrap().contains("{{filename}}")));

    document["trash"]["days"] = json!(12);
    document["ffmpeg"]["realtime"]["enabled"] = json!(true);
    document["oauth"]["enabled"] = json!(true);
    document["server"]["externalDomain"] = json!("https://photos.example/library");
    document["machineLearning"]["urls"] = json!(["http://saved.example:3003"]);
    document["storageTemplate"]["template"] =
        json!("{{y}}/{{#if album}}{{album}}{{else}}Other/{{MM}}{{/if}}/{{filename}}");
    let saved = call(
        &app,
        request(
            "PUT",
            "/api/system-config",
            Some(&session),
            Some(&document.to_string()),
        ),
    )
    .await;
    let saved_body = json_body(&saved);
    assert_eq!(saved.status, 200, "{saved_body}");
    assert_eq!(saved_body["trash"]["days"], 12);
    assert_eq!(
        saved_body["server"]["externalDomain"],
        "https://photos.example"
    );
    assert_eq!(
        saved_body["machineLearning"]["urls"][0],
        "http://saved.example:3003"
    );

    let again = call(
        &app,
        request("GET", "/api/admin/config", Some(&session), None),
    )
    .await;
    let again_body = json_body(&again);
    assert_eq!(again.status, 200, "{again_body}");
    assert_eq!(again_body["trash"]["days"], 12);
    assert_eq!(again_body["ffmpeg"]["realtime"]["enabled"], true);
    assert_eq!(again_body["oauth"]["enabled"], true);

    let defaults_again = call(
        &app,
        request("GET", "/api/system-config/defaults", Some(&session), None),
    )
    .await;
    let defaults_again_body = json_body(&defaults_again);
    assert_eq!(defaults_again_body["trash"]["days"], 30);
    assert_eq!(
        defaults_again_body["machineLearning"]["urls"][0],
        "http://ml.test:3003"
    );
    assert_eq!(defaults_again_body["oauth"]["enabled"], false);

    let features = call(&app, request("GET", "/api/server/features", None, None)).await;
    let features_body = json_body(&features);
    assert_eq!(features.status, 200, "{features_body}");
    assert_eq!(features_body["oauth"], true);
    assert_eq!(features_body["email"], false);
    assert_eq!(features_body["realtimeTranscoding"], false);
    assert_eq!(features_body["configFile"], false);

    let public = call(&app, request("GET", "/api/server/config", None, None)).await;
    let public_body = json_body(&public);
    assert_eq!(public_body["trashDays"], 12);
    assert_eq!(public_body["externalDomain"], "https://photos.example");
    assert_eq!(public_body["isOnboarded"], true);

    let mut rejected = again_body.clone();
    rejected["machineLearning"]["clip"]["modelName"] = json!("test-model");
    rejected["trash"]["days"] = json!(99);
    let bad_model = call(
        &app,
        request(
            "PUT",
            "/api/system-config",
            Some(&session),
            Some(&rejected.to_string()),
        ),
    )
    .await;
    let bad_model_body = json_body(&bad_model);
    assert_eq!(bad_model.status, 400, "{bad_model_body}");
    assert_eq!(
        bad_model_body["message"],
        "Unknown CLIP model: test-model. Please check the model name for typos and confirm this is a supported model."
    );
    let kept = call(
        &app,
        request("GET", "/api/system-config", Some(&session), None),
    )
    .await;
    let kept_body = json_body(&kept);
    assert_eq!(kept_body["trash"]["days"], 12);
    assert_eq!(
        kept_body["machineLearning"]["clip"]["modelName"],
        "ViT-B-32__openai"
    );

    let mut bad_template = kept_body.clone();
    bad_template["storageTemplate"]["template"] = json!("{{nope}}");
    let template = call(
        &app,
        request(
            "PUT",
            "/api/system-config",
            Some(&session),
            Some(&bad_template.to_string()),
        ),
    )
    .await;
    let template_body = json_body(&template);
    assert_eq!(template.status, 400, "{template_body}");
    assert_eq!(template_body["message"], "Invalid storage template");

    let mut smtp = kept_body.clone();
    smtp["notifications"]["smtp"]["enabled"] = json!(true);
    let mail = call(
        &app,
        request(
            "PUT",
            "/api/system-config",
            Some(&session),
            Some(&smtp.to_string()),
        ),
    )
    .await;
    let mail_body = json_body(&mail);
    assert_eq!(mail.status, 400, "{mail_body}");
    assert_eq!(mail_body["message"], "Invalid SMTP configuration");

    let mut logging = kept_body.clone();
    logging["logging"]["level"] = json!("verbose");
    let locked = call(
        &app,
        request(
            "PUT",
            "/api/system-config",
            Some(&session),
            Some(&logging.to_string()),
        ),
    )
    .await;
    let locked_body = json_body(&locked);
    assert_eq!(locked.status, 400, "{locked_body}");
    assert_eq!(
        locked_body["message"],
        "Logging cannot be changed while the environment variable IMMICH_LOG_LEVEL is set."
    );

    let test_mail = call(
        &app,
        request(
            "POST",
            "/api/admin/notifications/test-email",
            Some(&session),
            Some("{}"),
        ),
    )
    .await;
    let test_mail_body = json_body(&test_mail);
    assert_eq!(test_mail.status, 400, "{test_mail_body}");
    assert_eq!(
        test_mail_body["message"],
        "Failed to verify SMTP configuration"
    );

    sqlx::query("UPDATE user SET isAdmin = 0")
        .execute(app.pool())
        .await
        .unwrap();
    let forbidden = call(
        &app,
        request("GET", "/api/system-config", Some(&session), None),
    )
    .await;
    let forbidden_body = json_body(&forbidden);
    assert_eq!(forbidden.status, 403, "{forbidden_body}");
    assert_eq!(forbidden_body["message"], "Forbidden");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn config_file_replaces_the_saved_document() {
    let dir = std::env::temp_dir().join(format!("immich-config-file-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("immich.yaml");
    std::fs::write(&file, "trash:\n  days: 18\n").unwrap();
    let app = App::prepare(&config(&dir, Some(&file)))
        .await
        .expect("prepare");
    let session = sign_in(&app).await;

    let current = call(
        &app,
        request("GET", "/api/system-config", Some(&session), None),
    )
    .await;
    let body = json_body(&current);
    assert_eq!(current.status, 200, "{body}");
    assert_eq!(body["trash"]["days"], 18);

    let features = call(&app, request("GET", "/api/server/features", None, None)).await;
    assert_eq!(json_body(&features)["configFile"], true);

    let blocked = call(
        &app,
        request(
            "PUT",
            "/api/system-config",
            Some(&session),
            Some(&body.to_string()),
        ),
    )
    .await;
    let blocked_body = json_body(&blocked);
    assert_eq!(blocked.status, 400, "{blocked_body}");
    assert_eq!(
        blocked_body["message"],
        "Cannot update configuration while IMMICH_CONFIG_FILE is in use"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
