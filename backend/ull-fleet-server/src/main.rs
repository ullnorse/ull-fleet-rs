use tokio::net::TcpListener;
use tracing::info;

use ull_fleet_server::config::Config;
use ull_fleet_server::create_app;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();

    let config = Config::from_env()?;
    let listen_addr = config.listen_addr;
    let app = create_app(config)?;

    let listener = TcpListener::bind(listen_addr).await?;
    info!("fleet server listening on http://{listen_addr}");

    axum::serve(listener, app).await?;

    Ok(())
}
