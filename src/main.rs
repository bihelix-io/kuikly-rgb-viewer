mod server;

use anyhow::Result;
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("rgb_asset_viewer=info".parse()?),
        )
        .init();

    let addr = env::var("RGB_VIEWER_ADDR").unwrap_or_else(|_| "127.0.0.1:8092".to_string());
    server::serve(&addr).await
}
