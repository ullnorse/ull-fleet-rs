//! Admin login: a password from the environment, sessions in SQLite.

use axum::extract::{FromRequestParts, OriginalUri};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Redirect, Response};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::error::{AppError, AppResult};

pub const COOKIE_NAME: &str = "ull_fleet_session";
pub const SESSION_DAYS: i64 = 30;

/// Proof that the request may change the fleet. Pages redirect to the login
/// form when it is missing; API calls get 401.
pub struct Admin;

impl FromRequestParts<AppState> for Admin {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let Some(password) = state.config.admin_password.clone() else {
            return Ok(Admin);
        };
        if let Some(token) = bearer(&parts.headers)
            && constant_time_eq(&digest(token), &digest(&password))
        {
            return Ok(Admin);
        }
        if let Some(token) = session_cookie(&parts.headers) {
            let hash = token_hash(&token);
            let valid = state
                .db
                .call(move |conn| {
                    Ok(conn
                        .query_row(
                            "SELECT 1 FROM sessions WHERE token_hash = ?1 AND expires_at > ?2",
                            params![hash, crate::db::now()],
                            |_| Ok(()),
                        )
                        .optional()?
                        .is_some())
                })
                .await
                .map_err(IntoResponse::into_response)?;
            if valid {
                return Ok(Admin);
            }
        }

        let path = parts
            .extensions
            .get::<OriginalUri>()
            .map_or_else(|| parts.uri.clone(), |uri| uri.0.clone());
        if path.path().starts_with("/api/") {
            Err(AppError::Unauthorized.into_response())
        } else {
            let next = if parts.method == axum::http::Method::GET {
                path.path_and_query()
                    .map_or("/", |pq| pq.as_str())
                    .to_string()
            } else {
                "/".to_string()
            };
            Err(Redirect::to(&format!("/login?next={}", crate::ui::encode(&next))).into_response())
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value.to_string())
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn token_hash(token: &str) -> String {
    ull_fleet_sign::to_hex(&digest(token))
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn password_matches(state: &AppState, attempt: &str) -> bool {
    match &state.config.admin_password {
        Some(password) => constant_time_eq(&digest(attempt), &digest(password)),
        None => true,
    }
}

/// Creates a session and returns the `Set-Cookie` value.
pub async fn create_session(state: &AppState) -> AppResult<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|err| anyhow::anyhow!("getrandom failed: {err}"))?;
    let token = ull_fleet_sign::to_hex(&bytes);
    let hash = token_hash(&token);
    state
        .db
        .call(move |conn| {
            conn.execute(
                "INSERT INTO sessions (token_hash, expires_at) VALUES (?1, ?2)",
                params![hash, crate::db::now() + SESSION_DAYS * 86_400],
            )?;
            Ok(())
        })
        .await?;
    Ok(format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        SESSION_DAYS * 86_400
    ))
}

/// Deletes the request's session and returns a `Set-Cookie` that clears it.
pub async fn end_session(state: &AppState, headers: &HeaderMap) -> AppResult<String> {
    if let Some(token) = session_cookie(headers) {
        let hash = token_hash(&token);
        state
            .db
            .call(move |conn| {
                conn.execute("DELETE FROM sessions WHERE token_hash = ?1", [hash])?;
                Ok(())
            })
            .await?;
    }
    Ok(format!(
        "{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
    ))
}
