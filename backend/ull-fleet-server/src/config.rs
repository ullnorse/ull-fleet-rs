use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;

#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: SocketAddr,
    pub database_path: PathBuf,
    pub uploads_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen_addr = optional_env("LISTEN_ADDR")
            .unwrap_or_else(|| "0.0.0.0:3000".to_string())
            .parse()
            .context("failed to parse LISTEN_ADDR")?;

        Ok(Self {
            listen_addr,
            database_path: PathBuf::from(
                optional_env("DATABASE_PATH")
                    .unwrap_or_else(|| "backend/ull-fleet-server/data/fleet.sqlite3".to_string()),
            ),
            uploads_dir: PathBuf::from(
                optional_env("UPLOADS_DIR")
                    .unwrap_or_else(|| "backend/ull-fleet-server/data/uploads".to_string()),
            ),
        })
    }
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            built_env(name)
                .map(str::to_string)
                .filter(|value| !value.is_empty())
        })
}

fn built_env(name: &str) -> Option<&'static str> {
    match name {
        "LISTEN_ADDR" => option_env!("LISTEN_ADDR"),
        "DATABASE_PATH" => option_env!("DATABASE_PATH"),
        "UPLOADS_DIR" => option_env!("UPLOADS_DIR"),
        _ => None,
    }
}
