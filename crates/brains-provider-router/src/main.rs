use brains_provider_router::{app, RouterState};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "brains_provider_router=info,tower_http=info".into()),
        )
        .init();

    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .unwrap_or(8080);

    let llmkube_url = std::env::var("LLMKUBE_URL")
        .unwrap_or_else(|_| "http://brains-provider-llmkube.brains.svc.cluster.local:8000".to_string());

    let dgx_url = std::env::var("DGX_URL")
        .unwrap_or_else(|_| "http://brains-provider-dgx.brains.svc.cluster.local:8000".to_string());

    info!(
        port = port,
        llmkube_url = %llmkube_url,
        dgx_url = %dgx_url,
        "Starting Sovereign Brains Provider Router service"
    );

    let state = Arc::new(RouterState::new(&llmkube_url, &dgx_url));
    let app_router = app(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Listening on http://{}", addr);

    axum::serve(listener, app_router).await?;

    Ok(())
}
