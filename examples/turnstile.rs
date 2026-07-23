//! Headed Cloudflare Turnstile test against https://turnstile-test.vercel.app/
//!
//! Launches stealth Chromium headed, waits for the Turnstile widget to solve
//! itself (non-interactive), and reports whether a token was issued.
//!
//! Run with: `cargo run --example turnstile`

use cloakbrowser::{launch, LaunchOptions};
use futures::StreamExt;
use std::time::Duration;

const URL: &str = "https://turnstile-test.vercel.app/";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("launching headed Chromium ...");
    let (mut browser, mut handler) = launch(LaunchOptions {
        headless: false,
        ..Default::default()
    })
    .await?;

    // Keep draining handler events; do NOT break on a single errored event
    // (a failed subresource on a Cloudflare page must not tear down the session).
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    println!("opening blank page ...");
    let page = browser.new_page("about:blank").await?;

    println!("navigating to {URL} ...");
    page.goto(URL).await?;
    // Cloudflare pages pull many subresources; give the initial load a moment
    // and rely on token polling below.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // navigator.webdriver sanity check.
    let webdriver: serde_json::Value =
        page.evaluate("navigator.webdriver").await?.into_value()?;
    println!("navigator.webdriver = {webdriver}");

    // Turnstile writes the solved token into the hidden input
    // `input[name="cf-turnstile-response"]`. Poll it for up to ~30s.
    let mut token: String = String::new();
    for attempt in 1..=30 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let val: serde_json::Value = page
            .evaluate(
                r#"(() => {
                    const el = document.querySelector('input[name="cf-turnstile-response"]');
                    return el ? el.value : null;
                })()"#,
            )
            .await?
            .into_value()?;
        if let Some(t) = val.as_str() {
            if !t.is_empty() {
                token = t.to_string();
                println!("[{attempt}s] token issued ({} chars)", token.len());
                break;
            }
        }
        println!("[{attempt}s] waiting for Turnstile token ...");
    }

    // Grab any visible success/status text on the page for context.
    let body_text: String = page
        .evaluate("document.body ? document.body.innerText.slice(0, 400) : ''")
        .await?
        .into_value()?;

    page.save_screenshot(
        chromiumoxide::page::ScreenshotParams::builder().build(),
        "turnstile.png",
    )
    .await?;
    println!("saved turnstile.png");

    println!("\n=== page text (first 400 chars) ===\n{body_text}");
    println!(
        "\n=== verdict: {} ===",
        if token.is_empty() {
            "NO token (Turnstile did not solve)".to_string()
        } else {
            format!("SOLVED — token prefix: {}...", &token[..token.len().min(24)])
        }
    );

    // Keep the window up a few seconds so it's visible.
    tokio::time::sleep(Duration::from_secs(3)).await;

    browser.close().await?;
    handler_task.await?;
    Ok(())
}
