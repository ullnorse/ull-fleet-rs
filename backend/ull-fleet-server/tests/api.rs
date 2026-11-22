use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use tempfile::TempDir;
use tower::ServiceExt;

use ull_fleet_server::config::Config;
use ull_fleet_server::create_app;

const SEED: [u8; 32] = [7; 32];

fn signature_hex(image: &[u8]) -> String {
    ull_fleet_sign::to_hex(&ull_fleet_sign::sign_image(&SEED, image))
}

fn upload_request(image: &[u8], signature: Option<&str>) -> Request<Body> {
    let mut request =
        Request::post("/api/upload").header(header::CONTENT_TYPE, "application/octet-stream");
    if let Some(signature) = signature {
        request = request.header(ull_fleet_sign::SIGNATURE_HEADER, signature);
    }
    request
        .body(Body::from(image.to_vec()))
        .expect("upload request should be valid")
}

async fn body_text(response: axum::response::Response) -> String {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    String::from_utf8(body.to_vec()).expect("body should be utf-8")
}

#[tokio::test]
async fn upload_then_download_serves_the_update_once() {
    let fixture = TestApp::new();
    let app = fixture.app();
    let payload = b"ota-image".to_vec();

    let signature = signature_hex(&payload);
    let upload_response = app
        .clone()
        .oneshot(upload_request(&payload, Some(&signature)))
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
    assert_eq!(
        headers["x-application-image-sha256"],
        ull_fleet_sign::to_hex(&ull_fleet_sign::sha256(&payload)).as_str(),
    );
    assert_eq!(
        headers[ull_fleet_sign::SIGNATURE_HEADER],
        signature.as_str()
    );

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
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(ull_fleet_sign::SIGNATURE_HEADER, signature_hex(b""))
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

#[tokio::test]
async fn upload_rejects_non_binary_content_type() {
    let fixture = TestApp::new();
    let app = fixture.app();

    let response = app
        .oneshot(
            Request::post("/api/upload")
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("not-binary"))
                .expect("text upload request should be valid"),
        )
        .await
        .expect("text upload request should succeed");

    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("error body should be readable");

    assert_eq!(
        body.as_ref(),
        b"expected Content-Type: application/octet-stream"
    );
}

#[tokio::test]
async fn upload_requires_a_well_formed_signature() {
    let fixture = TestApp::new();
    let app = fixture.app();

    let missing = app
        .clone()
        .oneshot(upload_request(b"image", None))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    assert!(
        body_text(missing)
            .await
            .contains("missing x-application-image-ed25519")
    );

    let malformed = app
        .clone()
        .oneshot(upload_request(b"image", Some("abcd")))
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    assert!(body_text(malformed).await.contains("128 hex digits"));
}

#[tokio::test]
async fn configured_public_key_rejects_wrong_signatures() {
    let mut fixture = TestApp::new();
    fixture.config.ota_public_key = Some(ull_fleet_sign::public_key(&SEED));
    let app = fixture.app();

    let other_key_signature =
        ull_fleet_sign::to_hex(&ull_fleet_sign::sign_image(&[8; 32], b"image"));
    let rejected = app
        .clone()
        .oneshot(upload_request(b"image", Some(&other_key_signature)))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    assert!(body_text(rejected).await.contains("does not verify"));

    let accepted = app
        .oneshot(upload_request(b"image", Some(&signature_hex(b"image"))))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn database_without_signature_column_is_migrated() {
    let fixture = TestApp::new();
    let conn = rusqlite::Connection::open(&fixture.config.database_path).unwrap();
    conn.execute(
        "CREATE TABLE pending_update (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            filename TEXT NOT NULL,
            stored_path TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            sha256_hex TEXT NOT NULL,
            created_at TEXT NOT NULL
        )",
        [],
    )
    .unwrap();
    drop(conn);

    let app = fixture.app();
    let response = app
        .oneshot(upload_request(b"image", Some(&signature_hex(b"image"))))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
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
            ota_public_key: None,
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
