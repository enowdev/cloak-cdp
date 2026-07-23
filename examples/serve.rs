//! Run the cloakserve CDP multiplexer. Requires the `serve` feature.
//!
//! Run: cargo run --example serve --features serve
//! Then: curl 'http://127.0.0.1:9222/json/version?fingerprint=12345'

#[cfg(feature = "serve")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use cloakbrowser::serve::{run, ServeConfig};
    let port: u16 = std::env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(9222);
    println!("cloakserve listening on 127.0.0.1:{port}");
    run(ServeConfig {
        port,
        headless: true,
        ..Default::default()
    })
    .await?;
    Ok(())
}

#[cfg(not(feature = "serve"))]
fn main() {
    eprintln!("build with --features serve");
    std::process::exit(1);
}
