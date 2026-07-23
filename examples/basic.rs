//! Basic smoke test: launch stealth Chromium, hit a fingerprint page, screenshot.
//!
//! Run with: `cargo run --example basic`
//! Requires the stealth Chromium binary (auto-downloaded on first launch).

use cloakbrowser::{launch, LaunchOptions};
use futures::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (mut browser, mut handler) = launch(LaunchOptions {
        headless: false,
        ..Default::default()
    })
    .await?;

    let handler_task = tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    let page = browser
        .new_page("https://abrahamjuliot.github.io/creepjs/")
        .await?;
    page.wait_for_navigation().await?;

    // navigator.webdriver must be false for stealth to hold.
    let webdriver: serde_json::Value = page.evaluate("navigator.webdriver").await?.into_value()?;
    println!("navigator.webdriver = {webdriver}");

    page.save_screenshot(
        chromiumoxide::page::ScreenshotParams::builder().build(),
        "creepjs.png",
    )
    .await?;
    println!("saved creepjs.png");

    browser.close().await?;
    handler_task.await?;
    Ok(())
}
