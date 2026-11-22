use std::path::PathBuf;

use anyhow::Context;
use tokio::net::TcpListener;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use ull_fleet_server::config::Config;
use ull_fleet_server::{AppState, router, spawn_maintenance};

const USAGE: &str = "usage: ull-fleet-server [--env-file <path>]

Configuration comes from the environment (see .env.example). With --env-file,
variables are first loaded from that file; variables already set win.";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--env-file" => {
                let path = PathBuf::from(args.next().context(USAGE)?);
                dotenvy::from_path(&path)
                    .with_context(|| format!("failed to load {}", path.display()))?;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument {other:?}\n\n{USAGE}"),
        }
    }

    let config = Config::from_env()?;
    if config.admin_password.is_none() {
        warn!(
            "FLEET_ADMIN_PASSWORD is not set: anyone on the network can upload and deploy firmware"
        );
    }
    if config.ota_public_key.is_none() {
        warn!("OTA_PUBLIC_KEY is not set: uploads are not checked against your signing key");
    }
    let listen_addr = config.listen_addr;
    info!(data_dir = %config.data_dir.display(), "starting");
    let state = AppState::new(config)?;
    spawn_maintenance(state.clone());

    let listener = TcpListener::bind(listen_addr)
        .await
        .with_context(|| format!("failed to listen on {listen_addr}"))?;
    info!("fleet server listening on http://{listen_addr}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
