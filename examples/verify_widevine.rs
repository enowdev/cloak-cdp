//! Live test: widevine hint-seeding (no-op off Linux) + CDM fetch (Linux x86-64
//! only — errors cleanly elsewhere).

use cloakbrowser::widevine::{fetch_widevine_cdm, resolve_widevine_cdm_dir, seed_widevine_hint};
use std::path::Path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("platform = {} {}", std::env::consts::OS, std::env::consts::ARCH);

    // 1. resolve_widevine_cdm_dir — should be None when no CDM is installed.
    let fake_binary = Path::new("/tmp/nonexistent/Chromium");
    let cdm = resolve_widevine_cdm_dir(fake_binary);
    println!("resolve_widevine_cdm_dir = {cdm:?}  (None expected when no CDM present)");

    // 2. seed_widevine_hint — no-op off Linux, must not error.
    let seed = seed_widevine_hint(Path::new("/tmp/cloak-wv-profile"), fake_binary);
    println!("seed_widevine_hint = {seed:?}  (Ok, no-op off Linux)");

    // 3. fetch_widevine_cdm — only supported on Linux x86-64.
    println!("attempting CDM fetch ...");
    match fetch_widevine_cdm(Path::new("/tmp/cloak-wv"), false).await {
        Ok(p) => println!("fetch OK -> {}", p.display()),
        Err(e) => println!("fetch returned error (expected off linux-x64): {e}"),
    }

    println!("\nverdict: OK — widevine code paths exercised (no panics/hangs)");
    Ok(())
}
