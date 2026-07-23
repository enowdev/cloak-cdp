//! Basic smoke test: launch stealth Chromium, verify stealth signals.
//!
//! Run with: `cargo run --example basic`
//! Requires the stealth Chromium binary (auto-downloaded on first launch).

use cloakbrowser::{launch, LaunchOptions};
use futures::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let headless = std::env::var("HEADED").is_err();
    println!("launching (headless={headless}) ...");

    let (mut browser, mut handler) = launch(LaunchOptions {
        headless,
        ..Default::default()
    })
    .await?;

    // Drain handler events without tearing the session down on a single error.
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    println!("connected. opening page ...");
    let page = browser.new_page("about:blank").await?;

    // Stealth signals — read via the page's JS context.
    let webdriver: serde_json::Value = page.evaluate("navigator.webdriver").await?.into_value()?;
    let ua: String = page.evaluate("navigator.userAgent").await?.into_value()?;
    let platform: String = page.evaluate("navigator.platform").await?.into_value()?;
    let langs: serde_json::Value =
        page.evaluate("navigator.languages").await?.into_value()?;
    let hw: serde_json::Value = page
        .evaluate("navigator.hardwareConcurrency")
        .await?
        .into_value()?;

    println!("\n=== stealth signals ===");
    println!("navigator.webdriver         = {webdriver}   (must be false/null)");
    println!("navigator.platform          = {platform}");
    println!("navigator.hardwareConcurrency = {hw}");
    println!("navigator.languages         = {langs}");
    println!("navigator.userAgent         = {ua}");

    let webdriver_ok = webdriver.is_null() || webdriver == serde_json::Value::Bool(false);
    println!(
        "\n=== verdict: navigator.webdriver {} ===",
        if webdriver_ok { "OK (not automated)" } else { "LEAK!" }
    );

    browser.close().await?;
    handler_task.await?;

    if !webdriver_ok {
        anyhow::bail!("navigator.webdriver leaked automation");
    }
    Ok(())
}
