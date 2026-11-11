use std::net::IpAddr;
use std::path::Path;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
use immich_server::{App, Config};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

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
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, bytes.to_vec())
}

fn json_request(
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<&str>,
) -> Request<Body> {
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

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

fn assert_human_bytes(value: &Value, raw: u64) {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut magnitude = 0usize;
    let mut remainder = raw as f64;
    while remainder >= 1024.0 {
        if magnitude + 1 < UNITS.len() {
            magnitude += 1;
            remainder /= 1024.0;
        } else {
            break;
        }
    }
    let label = if magnitude == 0 {
        format!("{remainder:.0} {}", UNITS[magnitude])
    } else {
        format!("{remainder:.1} {}", UNITS[magnitude])
    };
    assert_eq!(value.as_str(), Some(label.as_str()), "{raw}");
}

async fn login(
    app: &App,
    email: &str,
    password: &str,
    user_agent: &str,
    model: &str,
    device: &str,
) -> (Value, String) {
    let (status, body) = call(
        app,
        Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .header("user-agent", user_agent)
            .header("devicemodel", model)
            .header("devicetype", device)
            .body(Body::from(format!(
                r#"{{"email":"{email}","password":"{password}"}}"#
            )))
            .unwrap(),
    )
    .await;
    let body = json_body(&body);
    assert_eq!(status, 201, "{body}");
    let token = body["accessToken"].as_str().expect("token").to_string();
    (body, format!("immich_access_token={token}"))
}

#[tokio::test]
async fn admin_creates_a_later_user() {
    let dir = std::env::temp_dir().join(format!("immich-users-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");

    let (status, body) = call(&app, json_request("GET", "/api/public/config", None, None)).await;
    let public_config = json_body(&body);
    assert_eq!(status, 200, "{public_config}");
    assert_eq!(public_config["passwordLogin"]["enabled"], true);
    assert_eq!(public_config["oauth"]["enabled"], false);
    assert_eq!(public_config["server"]["loginPageMessage"], "");
    assert!(public_config.get("job").is_none());

    let (status, body) = call(&app, json_request("GET", "/api/server/storage", None, None)).await;
    assert_eq!(status, 401, "{}", String::from_utf8_lossy(&body));

    let (_, admin_cookie) = login(
        &app,
        "admin@example.com",
        "correct horse",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/120.0.0.0 Safari/537.36",
        "Mac",
        "desktop",
    )
    .await;

    let (status, body) = call(
        &app,
        json_request("GET", "/api/server/storage", Some(&admin_cookie), None),
    )
    .await;
    let storage = json_body(&body);
    assert_eq!(status, 200, "{storage}");
    let total = storage["diskSizeRaw"].as_u64().expect("disk size");
    assert!(total > 0, "{storage}");
    assert_human_bytes(&storage["diskSize"], total);
    assert_human_bytes(
        &storage["diskUse"],
        storage["diskUseRaw"].as_u64().unwrap(),
    );
    assert_human_bytes(
        &storage["diskAvailable"],
        storage["diskAvailableRaw"].as_u64().unwrap(),
    );
    assert!(storage["diskUsagePercentage"].is_number());

    let (status, body) = call(
        &app,
        json_request("POST", "/api/admin/users", Some(&admin_cookie), Some("{}")),
    )
    .await;
    let missing = json_body(&body);
    assert_eq!(status, 400, "{missing}");
    assert_eq!(missing["message"], "password is required");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(&admin_cookie),
            Some(
                r#"{"email":"admin@example.com","password":"temp-pass","name":"Again","notify":true}"#,
            ),
        ),
    )
    .await;
    let duplicate = json_body(&body);
    assert_eq!(status, 400, "{duplicate}");
    assert_eq!(duplicate["message"], "Email is not available");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(&admin_cookie),
            Some(
                r#"{"email":"  Guest@Example.com ","password":"temp-pass","name":"Guest","notify":true,"quotaSizeInBytes":4096}"#,
            ),
        ),
    )
    .await;
    let created = json_body(&body);
    assert_eq!(status, 201, "{created}");
    let guest_id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["email"], "guest@example.com");
    assert_eq!(created["name"], "Guest");
    assert_eq!(created["profileImagePath"], "");
    assert!(created["deletedAt"].is_null());
    assert_eq!(created["oauthId"], "");
    assert!(created["storageLabel"].is_null());
    assert!(created["license"].is_null());
    assert_eq!(created["status"], "active");
    assert_eq!(created["quotaSizeInBytes"], 4096);
    assert_eq!(created["quotaUsageInBytes"], 0);
    assert_eq!(created["shouldChangePassword"], true);
    assert_eq!(created["isAdmin"], false);
    assert!(created["clusterGroupId"].is_null());
    assert!(created["createdAt"].as_str().unwrap().contains('T'));
    assert!(!created["avatarColor"].as_str().unwrap().is_empty());

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/admin/users?withDeleted=true",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let list = json_body(&body);
    assert_eq!(status, 200, "{list}");
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|user| user["email"] == "guest@example.com" && user["quotaSizeInBytes"] == 4096));

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/admin/users?withDeleted=maybe",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&body));

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users?id={guest_id}&withDeleted=true"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let found = json_body(&body);
    assert_eq!(status, 200, "{found}");
    assert_eq!(found.as_array().unwrap().len(), 1);
    assert_eq!(found[0]["id"], guest_id);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/preferences"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let preferences = json_body(&body);
    assert_eq!(status, 200, "{preferences}");
    assert_eq!(preferences["emailNotifications"]["enabled"], true);
    assert_eq!(preferences["folders"]["enabled"], false);
    assert_eq!(preferences["memories"]["enabled"], true);
    assert_eq!(preferences["people"]["enabled"], true);
    assert_eq!(preferences["ratings"]["enabled"], false);
    assert_eq!(preferences["sharedLinks"]["enabled"], true);
    assert_eq!(preferences["purchase"]["showSupportBadge"], true);
    assert_eq!(preferences["tags"]["enabled"], false);
    assert_eq!(preferences["cast"]["gCastEnabled"], false);

    insert_assets(&app, &guest_id).await;
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/statistics"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let stats = json_body(&body);
    assert_eq!(status, 200, "{stats}");
    assert_eq!(stats["images"], 1);
    assert_eq!(stats["videos"], 0);
    assert_eq!(stats["total"], 1);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!(
                "/api/admin/users/{guest_id}/calendar-heatmap?from=2020-01-01&to=2020-01-03&type=Upload"
            ),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let heatmap = json_body(&body);
    assert_eq!(status, 200, "{heatmap}");
    assert_eq!(heatmap["from"], "2020-01-01");
    assert_eq!(heatmap["to"], "2020-01-03");
    assert_eq!(heatmap["series"].as_array().unwrap().len(), 3);
    assert_eq!(heatmap["series"][0]["count"], 1);
    assert_eq!(heatmap["series"][1]["date"], "2020-01-02");
    assert_eq!(heatmap["series"][1]["count"], 1);
    assert_eq!(heatmap["series"][2]["count"], 0);
    assert_eq!(heatmap["totalCount"], 2);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/admin/users/00000000-0000-4000-8000-000000000000/calendar-heatmap?from=2020-01-01&to=2020-01-01",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let missing_user = json_body(&body);
    assert_eq!(status, 400, "{missing_user}");
    assert_eq!(missing_user["message"], "User not found");

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/sessions"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let sessions = json_body(&body);
    assert_eq!(status, 200, "{sessions}");
    assert_eq!(sessions.as_array().unwrap().len(), 0);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/users/{guest_id}/profile-image"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let image = json_body(&body);
    assert_eq!(status, 404, "{image}");
    assert_eq!(image["message"], "Not found");

    let (login_body, guest_cookie) = login(
        &app,
        "guest@example.com",
        "temp-pass",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/120.0.0.0 Safari/537.36",
        "Mac",
        "desktop",
    )
    .await;
    assert_eq!(login_body["shouldChangePassword"], true);
    assert_eq!(login_body["isOnboarded"], false);
    assert_eq!(login_body["isAdmin"], false);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(&guest_cookie),
            Some(r#"{"email":"other@example.com","password":"temp-pass","name":"Other"}"#),
        ),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/users/me",
            Some(&guest_cookie),
            Some(r#"{"password":""}"#),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let (status, body) = call(
        &app,
        json_request("GET", "/api/users/me", Some(&guest_cookie), None),
    )
    .await;
    let me = json_body(&body);
    assert_eq!(status, 200, "{me}");
    assert_eq!(me["shouldChangePassword"], true);

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/users/me",
            Some(&guest_cookie),
            Some(r#"{"password":"abc"}"#),
        ),
    )
    .await;
    let changed = json_body(&body);
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["shouldChangePassword"], false);
    let (status, _) = call(
        &app,
        json_request("POST", "/api/auth/validateToken", Some(&guest_cookie), None),
    )
    .await;
    assert_eq!(status, 200);

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/users/me/preferences",
            Some(&guest_cookie),
            Some(r#"{"cast":{"gCastEnabled":true}}"#),
        ),
    )
    .await;
    let merged = json_body(&body);
    assert_eq!(status, 200, "{merged}");
    assert_eq!(merged["cast"]["gCastEnabled"], true);
    assert_eq!(merged["memories"]["enabled"], true);
    assert_eq!(merged["people"]["minimumFaces"], 3);

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/users/me/onboarding",
            Some(&guest_cookie),
            Some(r#"{"isOnboarded":true}"#),
        ),
    )
    .await;
    let onboarded = json_body(&body);
    assert_eq!(status, 200, "{onboarded}");
    assert_eq!(onboarded["isOnboarded"], true);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/system-metadata/admin-onboarding",
            Some(&guest_cookie),
            Some(r#"{"isOnboarded":true}"#),
        ),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));

    let (second_login, ios_cookie) = login(
        &app,
        "guest@example.com",
        "abc",
        "immich-ios/2.0.0",
        "iPhone",
        "iOS",
    )
    .await;
    assert_eq!(second_login["shouldChangePassword"], false);
    assert_eq!(second_login["isOnboarded"], true);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/sessions"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let sessions = json_body(&body);
    assert_eq!(status, 200, "{sessions}");
    assert_eq!(sessions.as_array().unwrap().len(), 2);
    assert!(sessions.as_array().unwrap().iter().all(|row| {
        row["current"] == false
            && row["isPendingSyncReset"] == false
            && row.get("expiresAt").is_none()
            && row["deviceOS"].is_string()
            && row["deviceType"].is_string()
    }));
    let ios = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["deviceType"] == "iPhone")
        .expect("ios session");
    assert_eq!(ios["deviceOS"], "iOS");
    assert_eq!(ios["appVersion"], "2.0.0");
    let ios_id = ios["id"].as_str().unwrap().to_string();

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/auth/change-password",
            Some(&ios_cookie),
            Some(r#"{"password":"nope","newPassword":"long-enough","invalidateSessions":false}"#),
        ),
    )
    .await;
    let wrong = json_body(&body);
    assert_eq!(status, 400, "{wrong}");
    assert_eq!(wrong["message"], "Wrong password");

    let (status, _) = call(
        &app,
        json_request(
            "POST",
            "/api/auth/change-password",
            Some(&ios_cookie),
            Some(r#"{"password":"abc","newPassword":"short"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/auth/change-password",
            Some(&ios_cookie),
            Some(r#"{"password":"abc","newPassword":"long-enough","invalidateSessions":false}"#),
        ),
    )
    .await;
    let updated = json_body(&body);
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["shouldChangePassword"], false);
    let (status, _) = call(
        &app,
        json_request("POST", "/api/auth/validateToken", Some(&guest_cookie), None),
    )
    .await;
    assert_eq!(status, 401);
    let (status, _) = call(
        &app,
        json_request("POST", "/api/auth/validateToken", Some(&ios_cookie), None),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/sessions"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let sessions = json_body(&body);
    assert_eq!(status, 200, "{sessions}");
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["id"], ios_id);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/albums",
            Some(&admin_cookie),
            Some(r#"{"albumName":"Friends"}"#),
        ),
    )
    .await;
    let album = json_body(&body);
    assert_eq!(status, 201, "{album}");
    let album_id = album["id"].as_str().unwrap();
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/albums/{album_id}/users"),
            Some(&admin_cookie),
            Some(&format!(r#"{{"albumUsers":[{{"userId":"{guest_id}"}}]}}"#)),
        ),
    )
    .await;
    let shared = json_body(&body);
    assert_eq!(status, 200, "{shared}");
    assert!(shared["albumUsers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| { row["role"] == "editor" && row["user"]["id"] == guest_id }));
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/albums/{album_id}/users"),
            Some(&admin_cookie),
            Some(r#"{"albumUsers":[{"userId":"00000000-0000-4000-8000-000000000000"}]}"#),
        ),
    )
    .await;
    let invalid = json_body(&body);
    assert_eq!(status, 400, "{invalid}");
    assert_eq!(invalid["message"], "Invalid user");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/sync/stream",
            Some(&admin_cookie),
            Some(r#"{"types":["UsersV1"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let lines: Vec<Value> = String::from_utf8(body)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let users: Vec<&Value> = lines
        .iter()
        .filter(|line| line["type"] == "UserV1")
        .collect();
    assert!(users
        .iter()
        .any(|line| line["data"]["email"] == "admin@example.com"));
    let guest = users
        .iter()
        .find(|line| line["data"]["email"] == "guest@example.com")
        .expect("guest sync row");
    assert_eq!(guest["data"]["id"], guest_id);
    assert_eq!(guest["data"]["name"], "Guest");
    assert_eq!(guest["data"]["hasProfileImage"], false);
    assert!(guest["data"]["deletedAt"].is_null());
    assert!(lines.iter().any(|line| line["type"] == "SyncCompleteV1"));

    let (status, _) = call(
        &app,
        json_request(
            "POST",
            "/api/system-metadata/admin-onboarding",
            Some(&admin_cookie),
            Some(r#"{"isOnboarded":true}"#),
        ),
    )
    .await;
    assert_eq!(status, 204);
    let (status, body) = call(&app, json_request("GET", "/api/server/config", None, None)).await;
    let server = json_body(&body);
    assert_eq!(status, 200, "{server}");
    assert_eq!(server["isOnboarded"], true);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn admin_updates_deletes_and_restores_a_user() {
    let dir = std::env::temp_dir().join(format!("immich-users-delete-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let (_, admin_cookie) = login(
        &app,
        "admin@example.com",
        "correct horse",
        "Mozilla/5.0 Chrome/120.0.0.0",
        "Mac",
        "desktop",
    )
    .await;
    let admin_id = user_id(&app, &admin_cookie).await;

    let (status, body) = call(&app, json_request("GET", "/api/server/config", None, None)).await;
    let server = json_body(&body);
    assert_eq!(status, 200, "{server}");
    assert_eq!(server["userDeleteDelay"], 7);

    let guest_id = create_named(
        &app,
        &admin_cookie,
        "guest@example.com",
        "Guest",
        "temp-pass",
    )
    .await;
    let other_id = create_named(
        &app,
        &admin_cookie,
        "other@example.com",
        "Other",
        "temp-pass",
    )
    .await;
    let (_, guest_cookie) = login(
        &app,
        "guest@example.com",
        "temp-pass",
        "Mozilla/5.0 Chrome/120.0.0.0",
        "Mac",
        "desktop",
    )
    .await;

    let (status, _) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            None,
            Some(r#"{"name":"Nope"}"#),
        ),
    )
    .await;
    assert_eq!(status, 401);
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{admin_id}"),
            Some(&guest_cookie),
            Some(r#"{"name":"Nope"}"#),
        ),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/admin/users/not-a-uuid",
            Some(&admin_cookie),
            Some(r#"{"name":"Nope"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body)["message"]);
    assert_eq!(json_body(&body)["message"], "Validation failed");

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"pinCode":"12345"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"quotaSizeInBytes":-1}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"quotaSizeInBytes":1.5}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));

    let asset_id = insert_original(&app, &dir, &guest_id).await;
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"quotaSizeInBytes":500}"#),
        ),
    )
    .await;
    let quoted = json_body(&body);
    assert_eq!(status, 200, "{quoted}");
    assert_eq!(quoted["quotaSizeInBytes"], 500);
    assert_eq!(quoted["quotaUsageInBytes"], 100);
    assert!(quoted.get("password").is_none());
    assert!(quoted.get("pinCode").is_none());

    let (status, body) = call(
        &app,
        json_request(
            "PATCH",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"email":" Guest.Two@Example.com ","name":"Renamed","storageLabel":"a/b:c"}"#),
        ),
    )
    .await;
    let renamed = json_body(&body);
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(renamed["email"], "guest.two@example.com");
    assert_eq!(renamed["name"], "Renamed");
    assert_eq!(renamed["storageLabel"], "abc");

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"email":"admin@example.com"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(json_body(&body)["message"], "Email is not available");
    let (status, _) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{other_id}"),
            Some(&admin_cookie),
            Some(r#"{"storageLabel":"taken"}"#),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"storageLabel":"taken"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(
        json_body(&body)["message"],
        "Storage label already in use by another account"
    );
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"storageLabel":""}"#),
        ),
    )
    .await;
    let cleared = json_body(&body);
    assert_eq!(status, 200, "{cleared}");
    assert!(cleared["storageLabel"].is_null());

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"password":"abcdefghijklmnop","shouldChangePassword":true}"#),
        ),
    )
    .await;
    let reset = json_body(&body);
    assert_eq!(status, 200, "{reset}");
    assert_eq!(reset["shouldChangePassword"], true);
    let (_, guest_cookie) = login(
        &app,
        "guest.two@example.com",
        "abcdefghijklmnop",
        "Mozilla/5.0 Chrome/120.0.0.0",
        "Mac",
        "desktop",
    )
    .await;

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"pinCode":"123456"}"#),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", json_body(&body));
    let pin = auth_pin(&app, &guest_cookie).await;
    assert!(pin.as_str().unwrap().starts_with("$2"), "{pin}");
    let (status, _) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"pinCode":null}"#),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert!(auth_pin(&app, &guest_cookie).await.is_null());

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{admin_id}"),
            Some(&admin_cookie),
            Some(r#"{"isAdmin":false}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(
        json_body(&body)["message"],
        "Admin status can only be changed by another admin"
    );
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"isAdmin":true}"#),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", json_body(&body));
    assert_eq!(json_body(&body)["isAdmin"], true);
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&guest_cookie),
            Some(r#"{"isAdmin":false}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"isAdmin":false}"#),
        ),
    )
    .await;
    let demoted = json_body(&body);
    assert_eq!(status, 200, "{demoted}");
    assert_eq!(demoted["isAdmin"], false);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/albums",
            Some(&guest_cookie),
            Some(r#"{"albumName":"Trip"}"#),
        ),
    )
    .await;
    let album = json_body(&body);
    assert_eq!(status, 201, "{album}");
    let album_id = album["id"].as_str().unwrap().to_string();
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/albums/{album_id}/users"),
            Some(&guest_cookie),
            Some(&format!(r#"{{"albumUsers":[{{"userId":"{admin_id}"}}]}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", json_body(&body));
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/activities",
            Some(&guest_cookie),
            Some(&format!(
                r#"{{"albumId":"{album_id}","type":"comment","comment":"hello"}}"#
            )),
        ),
    )
    .await;
    assert_eq!(status, 201, "{}", json_body(&body));
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/shared-links",
            Some(&guest_cookie),
            Some(&format!(
                r#"{{"type":"ALBUM","albumId":"{album_id}","slug":"trip"}}"#
            )),
        ),
    )
    .await;
    let album_link = json_body(&body);
    assert_eq!(status, 201, "{album_link}");
    let album_key = album_link["key"].as_str().unwrap().to_string();
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/shared-links",
            Some(&guest_cookie),
            Some(&format!(
                r#"{{"type":"INDIVIDUAL","assetIds":["{asset_id}"],"slug":"one"}}"#
            )),
        ),
    )
    .await;
    let asset_link = json_body(&body);
    assert_eq!(status, 201, "{asset_link}");
    let asset_key = asset_link["key"].as_str().unwrap().to_string();

    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/admin/users/{admin_id}"),
            Some(&admin_cookie),
            Some(r#"{"force":false}"#),
        ),
    )
    .await;
    assert_eq!(status, 403, "{}", json_body(&body));
    assert_eq!(
        json_body(&body)["message"],
        "Cannot delete your own account"
    );
    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"force":false}"#),
        ),
    )
    .await;
    let removed = json_body(&body);
    assert_eq!(status, 200, "{removed}");
    assert_eq!(removed["status"], "deleted");
    assert!(removed["deletedAt"].is_string());

    let (status, _) = call(
        &app,
        json_request(
            "POST",
            "/api/auth/login",
            None,
            Some(r#"{"email":"guest.two@example.com","password":"abcdefghijklmnop"}"#),
        ),
    )
    .await;
    assert_eq!(status, 401);
    let (status, _) = call(
        &app,
        json_request("POST", "/api/auth/validateToken", Some(&guest_cookie), None),
    )
    .await;
    assert_eq!(status, 401);
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/sessions"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", json_body(&body));
    assert_eq!(json_body(&body).as_array().unwrap().len(), 0);
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/statistics"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", json_body(&body));
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/preferences"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", json_body(&body));
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/calendar-heatmap"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(json_body(&body)["message"], "User not found");
    assert!(!album_ids(&app, &admin_cookie).await.contains(&album_id));
    let (status, _) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/shared-links/me?key={album_key}"),
            None,
            None,
        ),
    )
    .await;
    assert_ne!(status, 200);
    let (status, _) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/shared-links/me?key={asset_key}"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/activities?albumId={album_id}"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let comments = json_body(&body);
    assert_eq!(status, 200, "{comments}");
    assert!(comments.as_array().unwrap().is_empty(), "{comments}");
    let synced = sync_lines(&app, &admin_cookie, r#"{"types":["UsersV1"]}"#).await;
    let guest_row = synced
        .iter()
        .find(|line| line["type"] == "UserV1" && line["data"]["id"] == guest_id)
        .expect("guest");
    assert!(guest_row["data"]["deletedAt"].is_string());

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/admin/users/{guest_id}/restore"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let restored = json_body(&body);
    assert_eq!(status, 200, "{restored}");
    assert!(restored["deletedAt"].is_null());
    assert_eq!(restored["status"], "active");
    let _ = login(
        &app,
        "guest.two@example.com",
        "abcdefghijklmnop",
        "Mozilla/5.0 Chrome/120.0.0.0",
        "Mac",
        "desktop",
    )
    .await;
    assert!(album_ids(&app, &admin_cookie).await.contains(&album_id));
    let (status, _) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/shared-links/me?key={album_key}"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/activities?albumId={album_id}"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let comments = json_body(&body);
    assert_eq!(status, 200, "{comments}");
    assert_eq!(comments.as_array().unwrap().len(), 1, "{comments}");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = immich_server::router(app.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, server).await;
    });
    let mut request = format!("ws://{addr}/api/socket.io/?EIO=4&transport=websocket")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", admin_cookie.parse().unwrap());
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

    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/admin/users/{guest_id}"),
            Some(&admin_cookie),
            Some(r#"{"force":true}"#),
        ),
    )
    .await;
    let forced = json_body(&body);
    assert_eq!(status, 200, "{forced}");
    assert_eq!(forced["status"], "removing");
    assert!(forced["deletedAt"].is_string());
    let event = next_text(&mut socket).await;
    assert!(event.contains("on_user_delete"), "{event}");
    assert!(event.contains(&guest_id), "{event}");
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user WHERE id = ?")
        .bind(&guest_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(remaining, 0);
    let assets: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset WHERE id = ?")
        .bind(&asset_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(assets, 0);
    assert!(!dir
        .join("objects")
        .join(format!("{guest_id}/{asset_id}/original"))
        .exists());
    assert!(!dir.join("thumbs").join(&asset_id).exists());
    assert!(!dir.join("profile").join(format!("{guest_id}.jpg")).exists());
    assert!(!dir.join("encoded").join(format!("{asset_id}.mp4")).exists());
    let deleted = sync_lines(
        &app,
        &admin_cookie,
        r#"{"types":["UsersV1","AlbumsV2","AssetsV2"]}"#,
    )
    .await;
    assert!(deleted
        .iter()
        .any(|line| { line["type"] == "UserDeleteV1" && line["data"]["userId"] == guest_id }));
    assert!(deleted
        .iter()
        .any(|line| { line["type"] == "AlbumDeleteV1" && line["data"]["albumId"] == album_id }));
    assert!(deleted.iter().all(|line| line["type"] != "AssetDeleteV1"));

    let stale_id = create_named(
        &app,
        &admin_cookie,
        "stale@example.com",
        "Stale",
        "temp-pass",
    )
    .await;
    let (status, _) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/admin/users/{stale_id}"),
            Some(&admin_cookie),
            Some(r#"{"force":false}"#),
        ),
    )
    .await;
    assert_eq!(status, 200);
    sqlx::query("UPDATE user SET deletedAt = '2020-01-01T00:00:00.000Z' WHERE id = ?")
        .bind(&stale_id)
        .execute(app.pool())
        .await
        .unwrap();
    let fresh_id = create_named(
        &app,
        &admin_cookie,
        "fresh@example.com",
        "Fresh",
        "temp-pass",
    )
    .await;
    let (status, _) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/admin/users/{fresh_id}"),
            Some(&admin_cookie),
            Some(r#"{"force":false}"#),
        ),
    )
    .await;
    assert_eq!(status, 200);
    app.purge_expired_users().await.expect("purge");
    let stale_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user WHERE id = ?")
        .bind(&stale_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    let fresh_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user WHERE id = ?")
        .bind(&fresh_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(stale_left, 0);
    assert_eq!(fresh_left, 1);

    let _ = std::fs::remove_dir_all(&dir);
}

async fn user_id(app: &App, cookie: &str) -> String {
    let (status, body) = call(
        app,
        json_request("GET", "/api/users/me", Some(cookie), None),
    )
    .await;
    let me = json_body(&body);
    assert_eq!(status, 200, "{me}");
    me["id"].as_str().unwrap().to_string()
}

async fn create_named(app: &App, cookie: &str, email: &str, name: &str, password: &str) -> String {
    let (status, body) = call(
        app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(cookie),
            Some(&format!(
                r#"{{"email":"{email}","password":"{password}","name":"{name}"}}"#
            )),
        ),
    )
    .await;
    let created = json_body(&body);
    assert_eq!(status, 201, "{created}");
    created["id"].as_str().unwrap().to_string()
}

async fn insert_original(app: &App, dir: &Path, owner_id: &str) -> String {
    let asset_id = Uuid::new_v4().to_string();
    let key = format!("{owner_id}/{asset_id}/original");
    let object = dir.join("objects").join(&key);
    std::fs::create_dir_all(object.parent().unwrap()).unwrap();
    std::fs::write(&object, b"original-bytes").unwrap();
    let thumb = dir.join("thumbs").join(&asset_id);
    std::fs::create_dir_all(&thumb).unwrap();
    std::fs::write(thumb.join("thumbnail.jpg"), b"thumb").unwrap();
    let profile = dir.join("profile").join(format!("{owner_id}.jpg"));
    std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
    std::fs::write(&profile, b"face").unwrap();
    let encoded = dir.join("encoded").join(format!("{asset_id}.mp4"));
    std::fs::create_dir_all(encoded.parent().unwrap()).unwrap();
    std::fs::write(&encoded, b"video").unwrap();
    let now = "2020-06-01T00:00:00.000Z";
    sqlx::query(
        "INSERT INTO asset (
            id, ownerId, type, originalPath, originalFileName, originalMimeType,
            fileCreatedAt, fileModifiedAt, localDateTime, checksum, visibility,
            createdAt, updatedAt, updateId
        ) VALUES (?, ?, 'IMAGE', ?, 'a.jpg', 'image/jpeg', ?, ?, ?, ?, 'timeline', ?, ?, ?)",
    )
    .bind(&asset_id)
    .bind(owner_id)
    .bind(&key)
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(vec![9_u8, 9, 9])
    .bind(now)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO asset_exif (assetId, fileSizeInByte, description, updatedAt, updateId)
         VALUES (?, 100, '', ?, ?)",
    )
    .bind(&asset_id)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO asset_file (id, assetId, type, path) VALUES (?, ?, 'encoded-video', ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&asset_id)
    .bind(format!("encoded/{asset_id}.mp4"))
    .execute(app.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE user SET profileImagePath = ? WHERE id = ?")
        .bind(format!("profile/{owner_id}.jpg"))
        .bind(owner_id)
        .execute(app.pool())
        .await
        .unwrap();
    asset_id
}

async fn auth_pin(app: &App, cookie: &str) -> Value {
    let lines = sync_lines(app, cookie, r#"{"types":["AuthUsersV1"]}"#).await;
    let row = lines
        .iter()
        .find(|line| line["type"] == "AuthUserV1")
        .expect("auth user");
    row["data"]["pinCode"].clone()
}

async fn album_ids(app: &App, cookie: &str) -> Vec<String> {
    let (status, body) = call(app, json_request("GET", "/api/albums", Some(cookie), None)).await;
    let albums = json_body(&body);
    assert_eq!(status, 200, "{albums}");
    albums
        .as_array()
        .unwrap()
        .iter()
        .map(|album| album["id"].as_str().unwrap().to_string())
        .collect()
}

async fn sync_lines(app: &App, cookie: &str, body: &str) -> Vec<Value> {
    let (status, bytes) = call(
        app,
        json_request("POST", "/api/sync/stream", Some(cookie), Some(body)),
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

async fn next_text(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> String {
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .expect("socket timed out")
        .expect("socket closed")
        .expect("socket error");
    message.to_text().expect("text").to_string()
}

async fn insert_assets(app: &App, owner_id: &str) {
    for (created_at, visibility, deleted_at, mark) in [
        ("2020-01-02T15:00:00.000Z", "timeline", None, 1_u8),
        ("2020-01-01T15:00:00.000Z", "hidden", None, 2),
        (
            "2020-01-03T15:00:00.000Z",
            "timeline",
            Some("2020-01-04T00:00:00.000Z"),
            3,
        ),
    ] {
        sqlx::query(
            "INSERT INTO asset (
                id, ownerId, type, originalPath, originalFileName, originalMimeType,
                fileCreatedAt, fileModifiedAt, localDateTime, checksum, visibility,
                deletedAt, createdAt, updatedAt, updateId
            ) VALUES (?, ?, 'IMAGE', 'object', 'a.jpg', 'image/jpeg', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(owner_id)
        .bind(created_at)
        .bind(created_at)
        .bind(created_at)
        .bind(vec![mark])
        .bind(visibility)
        .bind(deleted_at)
        .bind(created_at)
        .bind(created_at)
        .bind(Uuid::now_v7().to_string())
        .execute(app.pool())
        .await
        .expect("asset");
    }
}
