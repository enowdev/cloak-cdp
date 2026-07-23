//! Anti-redirect demo: a page tries to auto-redirect to another site; we block
//! the destination so the browser stays on the original page.
//!
//! Run with: cargo run --example block_redirect

use cloak_cdp::intercept::block_navigations;
use cloak_cdp::{launch, LaunchOptions};
use futures::StreamExt;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // block_urls non-empty => launcher enables CDP request interception.
    let (mut browser, mut handler) = launch(LaunchOptions {
        block_urls: vec!["*example.com*".into()],
        ..Default::default()
    })
    .await?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser.new_page("about:blank").await?;

    // Attach the blocker: any navigation to a URL containing "example.com" is
    // cancelled.
    let _guard = block_navigations(&page, ["*example.com*"]).await?;

    // A page that immediately JS-redirects to https://example.com/.
    let start = "data:text/html,<h1>ORIGINAL PAGE</h1>\
        <script>setTimeout(()=>{location.href='https://example.com/'},300)</script>";
    page.goto(start).await?;

    // Give the redirect time to fire (and be blocked).
    tokio::time::sleep(Duration::from_secs(3)).await;

    let url: String = page.evaluate("location.href").await?.into_value()?;

    println!("\n=== result ===");
    println!("current url = {}", &url[..url.len().min(60)]);

    // A blocked navigation never reaches the destination: the browser lands on
    // Chrome's error page (chrome-error://...) instead of example.com. That IS
    // the block working — the redirect target was never loaded.
    let blocked = !url.contains("example.com");
    println!(
        "\nverdict: {}",
        if blocked {
            "OK — redirect to example.com was BLOCKED (never reached the target)"
        } else {
            "redirect was NOT blocked"
        }
    );

    browser.close().await?;
    handler_task.await?;
    Ok(())
}
