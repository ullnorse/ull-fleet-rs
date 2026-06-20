use anyhow::Context;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};

use crate::api::MAX_UPLOAD_BYTES;
use crate::domain::ota::{OtaService, PendingUpdate, ServedUpdate};
use crate::error::AppResult;

pub async fn hello() -> &'static str {
    "hello from ull-fleet-server"
}

#[axum::debug_handler]
pub async fn index(State(ota_service): State<OtaService>) -> AppResult<Html<String>> {
    let pending = ota_service.pending_update()?;
    Ok(Html(index_html(pending.as_ref())))
}

#[axum::debug_handler]
pub async fn upload_update(
    State(ota_service): State<OtaService>,
    request: Request,
) -> AppResult<(StatusCode, String)> {
    let bytes = to_bytes(request.into_body(), MAX_UPLOAD_BYTES)
        .await
        .context("failed to read upload body")?;

    let pending = ota_service.upload(bytes.as_ref())?;

    Ok((
        StatusCode::CREATED,
        format!(
            "stored pending OTA image {} ({} bytes)",
            pending.filename, pending.size_bytes,
        ),
    ))
}

#[axum::debug_handler]
pub async fn download_update(State(ota_service): State<OtaService>) -> AppResult<Response> {
    let Some(served_update) = ota_service.take_pending_update()? else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };

    download_response(served_update).map_err(Into::into)
}

fn download_response(served_update: ServedUpdate) -> anyhow::Result<Response> {
    let content_length = HeaderValue::from_str(&served_update.bytes.len().to_string())
        .context("failed to build Content-Length header")?;
    let sha256_header = HeaderValue::from_str(&served_update.pending.sha256_hex)
        .context("failed to build x-application-image-sha256 header")?;

    let mut response = Response::new(Body::from(served_update.bytes));
    *response.status_mut() = StatusCode::OK;

    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CONTENT_LENGTH, content_length);
    headers.insert(
        HeaderName::from_static("x-application-image-sha256"),
        sha256_header,
    );

    Ok(response)
}

fn index_html(pending: Option<&PendingUpdate>) -> String {
    let pending_html = match pending {
        Some(pending) => format!(
            "<p>Pending update: <code>{}</code> ({} bytes, sha256 <code>{}</code>)</p>",
            pending.filename, pending.size_bytes, pending.sha256_hex,
        ),
        None => "<p>No pending OTA image.</p>".to_string(),
    };

    format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset=\"utf-8\">
  <title>ull-fleet-server</title>
</head>
<body>
  <h1>ull-fleet-server</h1>
  <p>Upload one OTA image. The ESP32 will poll <code>/api/update</code> every 10 seconds. When it downloads the image, the backend deletes it.</p>
  {pending_html}
  <form id=\"upload-form\">
    <input type=\"file\" id=\"ota-file\" required>
    <button type=\"submit\">Upload OTA Image</button>
  </form>
  <p id=\"upload-status\"></p>
  <p>API upload example:</p>
  <pre>curl --data-binary @ota.bin http://127.0.0.1:3000/api/upload</pre>
  <script>
    const form = document.getElementById('upload-form');
    const fileInput = document.getElementById('ota-file');
    const status = document.getElementById('upload-status');

    form.addEventListener('submit', async (event) => {{
      event.preventDefault();
      const file = fileInput.files[0];

      if (!file) {{
        status.textContent = 'Select a file first.';
        return;
      }}

      status.textContent = 'Uploading...';

      const response = await fetch('/api/upload', {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/octet-stream' }},
        body: file,
      }});

      status.textContent = await response.text();

      if (response.ok) {{
        setTimeout(() => window.location.reload(), 500);
      }}
    }});
  </script>
</body>
</html>"#,
        pending_html = pending_html,
    )
}
