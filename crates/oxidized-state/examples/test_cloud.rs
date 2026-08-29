//! Quick cloud connection test
//! Run with: cargo run --package oxidized-state --example test_cloud

use oxidized_state::{CloudConfig, SurrealHandle};

#[tokio::main]
async fn main() {
    // Load from environment
    dotenvy::dotenv().ok();

    println!("Testing SurrealDB Cloud connection...");

    match CloudConfig::from_env() {
        Ok(config) => {
            println!("  Endpoint: {}", config.endpoint);
            println!("  Namespace: {}", config.namespace);
            println!("  Database: {}", config.database);
            println!("  User: {}", config.username);
            println!("  Is Root: {}", config.is_root);

            match SurrealHandle::setup_cloud(config).await {
                Ok(_handle) => {
                    println!("\n✓ Successfully connected to SurrealDB Cloud!");
                    println!("✓ Schema initialized!");
                }
                Err(e) => {
                    eprintln!("\n✗ Connection failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("✗ Missing environment variables: {}", e);
            std::process::exit(1);
        }
    }
}
