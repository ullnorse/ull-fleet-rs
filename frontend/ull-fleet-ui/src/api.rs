//! Calls to the server's JSON API.

use gloo_net::http::{Request, RequestBuilder, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use ull_fleet_types::ApiError;
use web_sys::RequestCredentials;

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// Login required or the session expired.
    Unauthorized,
    /// The server refused with a message for the user.
    Server(String),
    /// No answer from the server.
    Network(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unauthorized => f.write_str("Sign in again"),
            Error::Server(message) => f.write_str(message),
            Error::Network(message) => write!(f, "Server unreachable ({message})"),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn prepare(builder: RequestBuilder) -> RequestBuilder {
    builder.credentials(RequestCredentials::SameOrigin)
}

async fn check(response: Response) -> Result<Response> {
    match response.status() {
        200..=299 => Ok(response),
        401 => Err(Error::Unauthorized),
        status => {
            let message = response
                .json::<ApiError>()
                .await
                .map(|body| body.error)
                .unwrap_or_else(|_| format!("HTTP {status}"));
            Err(Error::Server(upper_first(&message)))
        }
    }
}

fn upper_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn network(err: gloo_net::Error) -> Error {
    Error::Network(err.to_string())
}

pub async fn get<T: DeserializeOwned>(url: &str) -> Result<T> {
    let response = prepare(Request::get(url)).send().await.map_err(network)?;
    check(response).await?.json().await.map_err(network)
}

pub async fn send<B: Serialize, T: DeserializeOwned>(method: &str, url: &str, body: &B) -> Result<T> {
    let builder = match method {
        "POST" => Request::post(url),
        "PATCH" => Request::patch(url),
        "PUT" => Request::put(url),
        "DELETE" => Request::delete(url),
        _ => Request::get(url),
    };
    let response = prepare(builder)
        .json(body)
        .map_err(network)?
        .send()
        .await
        .map_err(network)?;
    check(response).await?.json().await.map_err(network)
}

/// For endpoints that answer 204.
pub async fn send_empty<B: Serialize>(method: &str, url: &str, body: Option<&B>) -> Result<()> {
    let builder = match method {
        "POST" => Request::post(url),
        "PATCH" => Request::patch(url),
        "DELETE" => Request::delete(url),
        _ => Request::get(url),
    };
    let builder = prepare(builder);
    let request = match body {
        Some(body) => builder.json(body).map_err(network)?,
        None => builder.build().map_err(network)?,
    };
    let response = request.send().await.map_err(network)?;
    check(response).await.map(|_| ())
}

/// Uploads a release image as the raw body.
pub async fn upload_release(
    file: &web_sys::File,
    signature: &str,
    notes: &str,
) -> Result<ull_fleet_types::ReleaseView> {
    let url = format!("/api/v1/releases?notes={}", encode(notes));
    let response = prepare(Request::post(&url))
        .header("x-application-image-ed25519", signature.trim())
        .header("content-type", "application/octet-stream")
        .body(wasm_bindgen::JsValue::from(file.clone()))
        .map_err(network)?
        .send()
        .await
        .map_err(network)?;
    check(response).await?.json().await.map_err(network)
}

pub fn encode(value: &str) -> String {
    js_sys::encode_uri_component(value).into()
}
