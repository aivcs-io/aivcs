//! Main entrypoint for `data-gateway` Axum microservice.

use data_gateway::{app, GatewayState};
use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://aivcs_admin@127.0.0.1:5432/aivcs_forge_v2".to_string());

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let cas_threshold: usize = std::env::var("CAS_THRESHOLD_BYTES")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(65536); // Default 64KB

    info!("Connecting to PostgreSQL pool...");
    let pool = PgPoolOptions::new()
        .max_connections(50)
        .connect(&database_url)
        .await?;

    let state = Arc::new(GatewayState::new(pool, cas_threshold));
    let app = app(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Starting data-gateway server on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
