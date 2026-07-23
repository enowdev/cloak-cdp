//! Live test: download + Ed25519-verify + extract the stealth binary into a
//! throwaway cache dir (set CLOAKBROWSER_CACHE_DIR before running).
//!
//! Run with:
//!   CLOAKBROWSER_CACHE_DIR=/tmp/cloak-dl-test cargo run --example verify_download

use cloak_cdp::download::ensure_binary;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cache = std::env::var("CLOAKBROWSER_CACHE_DIR").unwrap_or_default();
    println!("cache dir = {cache:?}");
    println!("downloading + verifying (Ed25519) + extracting ...");

    let t0 = std::time::Instant::now();
    let path = ensure_binary(None).await?;
    let dt = t0.elapsed();

    println!("\n=== OK ===");
    println!("binary       = {}", path.display());
    println!("exists       = {}", path.exists());
    println!("elapsed      = {:.1}s", dt.as_secs_f64());

    // Run --version to prove the extracted binary is a working executable.
    let out = std::process::Command::new(&path).arg("--version").output()?;
    println!(
        "chrome --version = {}",
        String::from_utf8_lossy(&out.stdout).trim()
    );
    Ok(())
}
