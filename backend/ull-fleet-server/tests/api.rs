use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;
use ull_fleet_server::config::Config;
use ull_fleet_server::{AppState, router};
use ull_fleet_sign::appdesc::test_image;
use ull_fleet_sign::{public_key, sign_image, to_hex};

const SEED: [u8; 32] = [7; 32];
const DEVICE: &str = "a1b2c3d4e5f6";
const PROJECT: &str = "sht3x-ssd1306";

struct TestApp {
    _dir: TempDir,
    app: Router,
}

impl TestApp {
    fn new(configure: impl FnOnce(&mut Config)) -> Self {
        let dir = TempDir::new().unwrap();
        let mut config = Config::for_tests(dir.path().to_path_buf());
        config.ota_public_key = Some(public_key(&SEED));
        configure(&mut config);
        let app = router(AppState::new(config).unwrap());
        Self { _dir: dir, app }
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, headers, body)
    }

    async fn checkin(&self, version: &str, uptime: u64, metrics: Value) -> Value {
        let body = json!({
            "project": PROJECT,
            "version": version,
            "secure_version": secure_version(version),
            "partition": "ota_0",
            "ota_state": "valid",
            "uptime_s": uptime,
            "rssi_dbm": -60,
            "ip": "192.168.1.20",
            "metrics": metrics,
        });
        let (status, _, body) = self
            .send(
                Request::post(format!("/api/v1/devices/{DEVICE}/checkin"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        serde_json::from_slice(&body).unwrap()
    }

    async fn upload(&self, image: &[u8], signature: &str) -> (StatusCode, Value) {
        let (status, _, body) = self
            .send(
                Request::post("/api/v1/releases")
                    .header(ull_fleet_sign::SIGNATURE_HEADER, signature)
                    .body(Body::from(image.to_vec()))
                    .unwrap(),
            )
            .await;
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn deploy(&self, release_id: i64, devices: Option<&[&str]>) -> (StatusCode, Value) {
        let body = match devices {
            Some(devices) => json!({ "devices": devices }),
            None => json!({}),
        };
        let (status, _, body) = self
            .send(
                Request::post(format!("/api/v1/releases/{release_id}/deploy"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await;
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn firmware(&self) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        self.send(
            Request::get(format!("/api/v1/devices/{DEVICE}/firmware"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn page(&self, path: &str) -> (StatusCode, String) {
        let (status, _, body) = self
            .send(Request::get(path).body(Body::empty()).unwrap())
            .await;
        (status, String::from_utf8(body).unwrap())
    }
}

fn secure_version(version: &str) -> u32 {
    version.rsplit('-').next().unwrap().parse().unwrap()
}

fn release(version: &str) -> (Vec<u8>, String) {
    let image = test_image(PROJECT, version, secure_version(version), 4096);
    let signature = to_hex(&sign_image(&SEED, &image));
    (image, signature)
}

#[tokio::test]
async fn checkin_registers_device_and_metrics() {
    let app = TestApp::new(|_| {});
    let reply = app
        .checkin(
            "0.1.0-100",
            5,
            json!({ "temperature_c": 22.5, "humidity_pct": 48.0 }),
        )
        .await;
    assert_eq!(reply, json!({ "update": false, "poll_interval_s": 30 }));

    let (status, html) = app.page("/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("22.5"), "{html}");
    assert!(html.contains("1 of 1 online"));

    let (status, html) = app.page(&format!("/devices/{DEVICE}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Temperature"));
    assert!(html.contains("<svg"));
    assert!(html.contains("First check-in"));
}

#[tokio::test]
async fn checkin_rejects_bad_input() {
    let app = TestApp::new(|_| {});
    for (path, body) in [
        (
            "/api/v1/devices/NOT-A-MAC/checkin",
            json!({"project": "p", "version": "1"}),
        ),
        (
            "/api/v1/devices/a1b2c3d4e5f6/checkin",
            json!({"project": "p", "version": "1", "metrics": {"Bad Name": 1.0}}),
        ),
        (
            "/api/v1/devices/a1b2c3d4e5f6/checkin",
            json!({"project": "", "version": "1"}),
        ),
    ] {
        let (status, _, _) = app
            .send(
                Request::post(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {body}");
    }
}

#[tokio::test]
async fn upload_validates_image_and_signature() {
    let app = TestApp::new(|_| {});
    let (image, signature) = release("0.2.0-200");

    let (status, _) = app.upload(&image, "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unsigned upload");

    let wrong = to_hex(&sign_image(&[9; 32], &image));
    let (status, _) = app.upload(&image, &wrong).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "wrong key");

    let mut not_esp = image.clone();
    not_esp[0] = 0;
    let (status, _) = app
        .upload(&not_esp, &to_hex(&sign_image(&SEED, &not_esp)))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "not an ESP image");

    let (status, body) = app.upload(&image, &signature).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["project"], PROJECT);
    assert_eq!(body["version"], "0.2.0-200");

    let (status, _) = app.upload(&image, &signature).await;
    assert_eq!(status, StatusCode::CONFLICT, "duplicate version");
}

#[tokio::test]
async fn update_flow_keeps_image_and_tracks_success() {
    let app = TestApp::new(|_| {});
    app.checkin("0.1.0-100", 50, json!({})).await;
    let (image, signature) = release("0.2.0-200");
    let (_, body) = app.upload(&image, &signature).await;
    let id = body["id"].as_i64().unwrap();

    let (status, _, _) = app.firmware().await;
    assert_eq!(status, StatusCode::NO_CONTENT, "nothing deployed yet");

    let (status, body) = app.deploy(id, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["targeted"], json!([DEVICE]));
    assert_eq!(
        app.checkin("0.1.0-100", 60, json!({})).await["update"],
        true
    );

    for _ in 0..2 {
        // A retried download gets the same image: nothing is consumed on serve.
        let (status, headers, body) = app.firmware().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, image);
        assert_eq!(headers[header::CONTENT_LENGTH], image.len().to_string());
        assert_eq!(
            headers[ull_fleet_sign::SIGNATURE_HEADER],
            signature.as_str()
        );
        assert_eq!(
            headers["x-application-image-sha256"],
            to_hex(&ull_fleet_sign::sha256(&image)).as_str()
        );
    }

    let reply = app.checkin("0.2.0-200", 3, json!({})).await;
    assert_eq!(reply["update"], false);
    let (status, _, _) = app.firmware().await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, html) = app.page(&format!("/devices/{DEVICE}")).await;
    assert!(html.contains("Updated to 0.2.0-200"), "{html}");
    assert!(html.contains("Up to date"));
}

#[tokio::test]
async fn failed_updates_stop_after_three_attempts() {
    let app = TestApp::new(|_| {});
    app.checkin("0.1.0-100", 50, json!({})).await;
    let (image, signature) = release("0.2.0-200");
    let (_, body) = app.upload(&image, &signature).await;
    app.deploy(body["id"].as_i64().unwrap(), Some(&[DEVICE]))
        .await;

    for attempt in 1..=3 {
        assert_eq!(
            app.checkin("0.1.0-100", 100, json!({})).await["update"],
            attempt <= 3
        );
        let (status, _, _) = app.firmware().await;
        assert_eq!(status, StatusCode::OK);
    }
    // Third failure: no longer offered.
    assert_eq!(
        app.checkin("0.1.0-100", 200, json!({})).await["update"],
        false
    );
    let (status, _, _) = app.firmware().await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, html) = app.page(&format!("/devices/{DEVICE}")).await;
    assert!(html.contains("Failed 3 times"), "{html}");
}

#[tokio::test]
async fn deploy_refuses_older_or_foreign_releases() {
    let app = TestApp::new(|_| {});
    app.checkin("0.3.0-300", 5, json!({})).await;

    let (image, signature) = release("0.2.0-200");
    let (_, body) = app.upload(&image, &signature).await;
    let (status, _) = app
        .deploy(body["id"].as_i64().unwrap(), Some(&[DEVICE]))
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "older than running firmware"
    );

    let other = test_image("other-project", "1.0.0-400", 400, 64);
    let (_, body) = app
        .upload(&other, &to_hex(&sign_image(&SEED, &other)))
        .await;
    let (status, _) = app
        .deploy(body["id"].as_i64().unwrap(), Some(&[DEVICE]))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "different project");
}

#[tokio::test]
async fn login_protects_ui_and_admin_api() {
    let app = TestApp::new(|config| config.admin_password = Some("hunter2".into()));
    app.checkin("0.1.0-100", 5, json!({})).await; // devices need no login

    let (status, headers, _) = app
        .send(Request::get("/").body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/login?next=/");

    let (image, signature) = release("0.2.0-200");
    let (status, _) = app.upload(&image, &signature).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, headers, _) = app
        .send(
            Request::post("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("password=wrong&next=%2F"))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(headers.get(header::SET_COOKIE).is_none());

    let (status, headers, _) = app
        .send(
            Request::post("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "password=hunter2&next=https%3A%2F%2Fevil.example",
                ))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/", "external redirect refused");
    let cookie = headers[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.contains("HttpOnly"));
    let session = cookie.split(';').next().unwrap().to_string();

    let (status, _, body) = app
        .send(
            Request::get("/")
                .header(header::COOKIE, &session)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains(DEVICE));

    let (status, _, _) = app
        .send(
            Request::get("/api/v1/devices")
                .header(header::AUTHORIZATION, "Bearer hunter2")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    app.send(
        Request::post("/logout")
            .header(header::COOKIE, &session)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let (status, _, _) = app
        .send(
            Request::get("/")
                .header(header::COOKIE, &session)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "session ended");
}

#[tokio::test]
async fn ui_upload_form_and_rename() {
    let app = TestApp::new(|_| {});
    app.checkin("0.1.0-100", 5, json!({ "temperature_c": 21.0 }))
        .await;
    let (image, signature) = release("0.2.0-200");

    let boundary = "XBOUNDARYX";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"fw.bin\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&image);
    body.extend_from_slice(
        format!(
            "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"signature\"\r\n\r\n{signature}\r\n\
             --{boundary}--\r\n"
        )
        .as_bytes(),
    );
    let (status, headers, _) = app
        .send(
            Request::post("/releases")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let location = headers[header::LOCATION].to_str().unwrap();
    assert!(
        location.starts_with("/releases?notice=Uploaded"),
        "{location}"
    );

    let (_, html) = app.page("/releases").await;
    assert!(html.contains("0.2.0-200"));
    assert!(html.contains("0 of 1 running"));

    let (status, _, _) = app
        .send(
            Request::post(format!("/devices/{DEVICE}/rename"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("name=Living+room"))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, html) = app.page("/").await;
    assert!(html.contains("Living room"));
}

#[tokio::test]
async fn assets_are_served() {
    let app = TestApp::new(|_| {});
    for path in ["/assets/style.css", "/assets/app.js"] {
        let (status, headers, body) = app
            .send(Request::get(path).body(Body::empty()).unwrap())
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body.is_empty());
        assert!(
            headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/")
        );
    }
}
