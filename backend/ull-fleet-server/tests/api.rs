use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tempfile::TempDir;
use tower::ServiceExt;

use ull_fleet_server::config::Config;
use ull_fleet_server::create_app;

#[tokio::test]
async fn upload_then_download_serves_the_update_once() {
    let fixture = TestApp::new();
    let app = fixture.app();
    let payload = b"ota-image".to_vec();

    let upload_response = app
        .clone()
        .oneshot(
            Request::post("/api/upload")
                .body(Body::from(payload.clone()))
                .expect("upload request should be valid"),
        )
        .await
        .expect("upload request should succeed");

    assert_eq!(upload_response.status(), StatusCode::CREATED);

    let download_response = app
        .clone()
        .oneshot(
            Request::get("/api/update")
                .body(Body::empty())
                .expect("download request should be valid"),
        )
        .await
        .expect("download request should succeed");

    assert_eq!(download_response.status(), StatusCode::OK);

    let headers = download_response.headers().clone();
    let body = to_bytes(download_response.into_body(), usize::MAX)
        .await
        .expect("download body should be readable");

    assert_eq!(body.as_ref(), payload.as_slice());
    assert_eq!(
        headers
            .get("content-type")
            .expect("content-type header should be present"),
        "application/octet-stream",
    );
    assert_eq!(
        headers
            .get("content-length")
            .expect("content-length header should be present"),
        payload.len().to_string().as_str(),
    );
    assert!(headers.contains_key("x-application-image-sha256"));

    let no_content_response = app
        .oneshot(
            Request::get("/api/update")
                .body(Body::empty())
                .expect("follow-up download request should be valid"),
        )
        .await
        .expect("follow-up download request should succeed");

    assert_eq!(no_content_response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn upload_rejects_empty_bodies() {
    let fixture = TestApp::new();
    let app = fixture.app();

    let response = app
        .oneshot(
            Request::post("/api/upload")
                .body(Body::empty())
                .expect("empty upload request should be valid"),
        )
        .await
        .expect("empty upload request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("error body should be readable");

    assert_eq!(body.as_ref(), b"uploaded file is empty");
}

struct TestApp {
    _temp_dir: TempDir,
    config: Config,
}

impl TestApp {
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("temporary directory should be created");
        let config = Config {
            listen_addr: "127.0.0.1:0".parse().expect("listen address should parse"),
            database_path: temp_dir.path().join("fleet.sqlite3"),
            uploads_dir: temp_dir.path().join("uploads"),
        };

        Self {
            _temp_dir: temp_dir,
            config,
        }
    }

    fn app(&self) -> Router {
        create_app(self.config.clone()).expect("test app should be created")
    }
}
