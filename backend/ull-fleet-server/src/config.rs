use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;

/// Runtime configuration, read from the environment at startup.
#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: SocketAddr,
    /// Directory for the SQLite database and release images.
    pub data_dir: PathBuf,
    /// When set, uploads must be signed by this key.
    pub ota_public_key: Option<[u8; 32]>,
    /// When set, the UI and admin API require a login.
    pub admin_password: Option<String>,
    /// How often devices are told to check in.
    pub poll_interval: Duration,
    pub metrics_retention_days: u32,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            listen_addr: var("LISTEN_ADDR")
                .unwrap_or_else(|| "0.0.0.0:3000".to_string())
                .parse()
                .context("failed to parse LISTEN_ADDR")?,
            data_dir: var("DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(default_data_dir),
            ota_public_key: var("OTA_PUBLIC_KEY")
                .map(|key| ull_fleet_sign::from_hex(&key))
                .transpose()
                .context("failed to parse OTA_PUBLIC_KEY")?,
            admin_password: var("FLEET_ADMIN_PASSWORD"),
            poll_interval: Duration::from_secs(
                var("POLL_INTERVAL_S")
                    .map(|value| value.parse())
                    .transpose()
                    .context("failed to parse POLL_INTERVAL_S")?
                    .unwrap_or(30),
            ),
            metrics_retention_days: var("METRICS_RETENTION_DAYS")
                .map(|value| value.parse())
                .transpose()
                .context("failed to parse METRICS_RETENTION_DAYS")?
                .unwrap_or(30),
        })
    }

    /// A config for tests: everything under `data_dir`, no auth, no key.
    pub fn for_tests(data_dir: PathBuf) -> Self {
        Self {
            listen_addr: "127.0.0.1:0".parse().unwrap(),
            data_dir,
            ota_public_key: None,
            admin_password: None,
            poll_interval: Duration::from_secs(30),
            metrics_retention_days: 30,
        }
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("fleet.sqlite3")
    }

    pub fn releases_dir(&self) -> PathBuf {
        self.data_dir.join("releases")
    }
}

fn var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn default_data_dir() -> PathBuf {
    let base = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("ull-fleet")
}
