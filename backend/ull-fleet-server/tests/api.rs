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

    async fn get(&self, path: &str) -> Value {
        let (status, _, body) = self
            .send(Request::get(path).body(Body::empty()).unwrap())
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{path}: {}",
            String::from_utf8_lossy(&body)
        );
        serde_json::from_slice(&body).unwrap()
    }

    async fn json(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let (status, _, body) = self
            .send(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await;
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn device(&self) -> Value {
        self.get(&format!("/api/v1/devices/{DEVICE}")).await
    }

    async fn event_messages(&self) -> Vec<String> {
        self.get(&format!("/api/v1/events?device={DEVICE}"))
            .await
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["message"].as_str().unwrap().to_string())
            .collect()
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

    let snapshot = app.get("/api/v1/snapshot").await;
    let device = &snapshot["devices"][0];
    assert_eq!(device["id"], DEVICE);
    assert_eq!(device["online"], true);
    assert_eq!(device["readings"][0]["name"], "temperature_c");
    assert_eq!(device["readings"][0]["value"], 22.5);
    assert!(
        snapshot["events"][0]["message"]
            .as_str()
            .unwrap()
            .starts_with("First check-in")
    );

    let series = app
        .get(&format!(
            "/api/v1/devices/{DEVICE}/series?metric=humidity_pct&range=1h"
        ))
        .await;
    assert_eq!(series["points"][0]["avg"], 48.0);
    assert_eq!(series["bucket_s"], 20);

    let lines = app.get("/api/v1/sparklines").await;
    assert_eq!(lines.as_array().unwrap().len(), 2);
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

    assert!(
        app.event_messages()
            .await
            .contains(&"Updated to 0.2.0-200".to_string())
    );
    let device = app.device().await;
    assert_eq!(device["update"]["state"], "none");
    assert_eq!(device["version"], "0.2.0-200");
    assert_eq!(app.get("/api/v1/releases").await[0]["running"], 1);
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

    let device = app.device().await;
    assert_eq!(device["update"]["state"], "failed");
    assert_eq!(device["update"]["attempts"], 3);
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
async fn login_protects_admin_api() {
    let app = TestApp::new(|config| config.admin_password = Some("hunter2".into()));
    app.checkin("0.1.0-100", 5, json!({})).await; // devices need no login

    // The app shell itself is public; its data is not.
    let (status, _, _) = app
        .send(Request::get("/devices/x").body(Body::empty()).unwrap())
        .await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = app
        .send(
            Request::get("/api/v1/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        app.get("/api/v1/session").await,
        json!({ "auth_required": true, "signed_in": false })
    );

    let (image, signature) = release("0.2.0-200");
    let (status, _) = app.upload(&image, &signature).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = app
        .json("POST", "/api/v1/login", json!({ "password": "wrong" }))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, headers, _) = app
        .send(
            Request::post("/api/v1/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "password": "hunter2" }).to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let cookie = headers[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.contains("HttpOnly"));
    let session = cookie.split(';').next().unwrap().to_string();

    let with_cookie = |path: &str| {
        Request::get(path)
            .header(header::COOKIE, &session)
            .body(Body::empty())
            .unwrap()
    };
    let (status, _, body) = app.send(with_cookie("/api/v1/snapshot")).await;
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
        Request::post("/api/v1/logout")
            .header(header::COOKIE, &session)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let (status, _, _) = app.send(with_cookie("/api/v1/snapshot")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "session ended");
}

#[tokio::test]
async fn rename_location_and_alerts() {
    let app = TestApp::new(|_| {});
    app.checkin("0.1.0-100", 5, json!({ "humidity_pct": 50.0 }))
        .await;

    let (status, device) = app
        .json(
            "PATCH",
            &format!("/api/v1/devices/{DEVICE}"),
            json!({ "name": " Living room ", "location": "Upstairs" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(device["name"], "Living room");
    assert_eq!(device["location"], "Upstairs");

    let (status, _) = app
        .json(
            "PATCH",
            &format!("/api/v1/devices/{DEVICE}"),
            json!({ "alert_rules": [{ "metric": "humidity_pct", "min": 60.0, "max": 40.0 }] }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "min above max");

    // A rule the latest reading already breaks fires at once.
    let (status, device) = app
        .json(
            "PATCH",
            &format!("/api/v1/devices/{DEVICE}"),
            json!({ "alert_rules": [{ "metric": "humidity_pct", "min": null, "max": 45.0 }] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(device["alerts"][0]["direction"], "above");
    assert!(app.event_messages().await[0].starts_with("Humidity above 45 %"));

    // Still high: no second alert event.
    app.checkin("0.1.0-100", 35, json!({ "humidity_pct": 52.0 }))
        .await;
    assert_eq!(app.device().await["alerts"][0]["value"], 52.0);
    let alerts = app
        .event_messages()
        .await
        .iter()
        .filter(|message| message.contains("above"))
        .count();
    assert_eq!(alerts, 1);

    app.checkin("0.1.0-100", 65, json!({ "humidity_pct": 41.0 }))
        .await;
    assert_eq!(app.device().await["alerts"], json!([]));
    assert_eq!(
        app.event_messages().await[0],
        "Humidity back in range: 41 %"
    );

    let (status, _, _) = app
        .send(
            Request::delete(format!("/api/v1/devices/{DEVICE}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(app.get("/api/v1/devices").await, json!([]));
}

#[tokio::test]
async fn stream_starts_with_snapshot_and_watching_speeds_up_checkins() {
    let app = TestApp::new(|_| {});
    app.checkin("0.1.0-100", 5, json!({})).await;

    let response = app
        .app
        .clone()
        .oneshot(Request::get("/api/v1/stream").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let frame = body.frame().await.unwrap().unwrap().into_data().unwrap();
    let text = String::from_utf8_lossy(&frame);
    assert!(text.starts_with("data: {\"type\":\"snapshot\""), "{text}");

    let watch = app
        .app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/devices/{DEVICE}/watch"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(watch.status(), StatusCode::OK);
    let reply = app.checkin("0.1.0-100", 10, json!({})).await;
    assert_eq!(reply["poll_interval_s"], 5);
    assert_eq!(app.device().await["live"], true);

    // The stream reports the check-in.
    let mut seen_device = false;
    for _ in 0..5 {
        let frame = body.frame().await.unwrap().unwrap().into_data().unwrap();
        if String::from_utf8_lossy(&frame).contains("\"type\":\"device\"") {
            seen_device = true;
            break;
        }
    }
    assert!(seen_device);

    drop(watch);
    tokio::task::yield_now().await;
    let reply = app.checkin("0.1.0-100", 15, json!({})).await;
    assert_eq!(reply["poll_interval_s"], 30);
}

#[tokio::test]
async fn ui_shell_is_served_for_client_routes() {
    let app = TestApp::new(|_| {});
    let (status, _, _) = app
        .send(Request::get("/api/v1/nope").body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // index.html, or the "not built" hint when frontend/dist is empty.
    let (status, _, _) = app
        .send(
            Request::get("/devices/a1b2c3d4e5f6")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert!(status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn download_progress_finishes_on_byte_count() {
    let app = TestApp::new(|_| {});
    app.checkin("0.1.0-100", 50, json!({})).await;
    let (image, signature) = release("0.2.0-200");
    let (_, body) = app.upload(&image, &signature).await;
    app.deploy(body["id"].as_i64().unwrap(), None).await;
    app.checkin("0.1.0-100", 60, json!({})).await;

    let response = app
        .app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/devices/{DEVICE}/firmware"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Read like hyper does with a Content-Length: stop after the last byte
    // without polling for the end of the stream, then drop the body.
    let mut body = response.into_body();
    let mut received = 0;
    while received < image.len() {
        let frame = body.frame().await.unwrap().unwrap();
        received += frame.into_data().unwrap().len();
        if received < image.len() {
            let progress = &app.device().await["update"]["progress"];
            assert_eq!(progress["done"], false);
            assert_eq!(progress["total"], image.len());
        }
    }
    drop(body);

    let progress = &app.device().await["update"]["progress"];
    assert_eq!(progress["sent"], image.len());
    assert_eq!(progress["done"], true);
}
