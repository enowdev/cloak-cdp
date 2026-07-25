//! Bandwidth saving demo: block images/CSS/fonts/media and measure the transfer
//! reduction on a real page. Useful to cut metered-proxy usage.
//!
//! Run with: cargo run --example block_resources

use cloak_cdp::intercept::{block_requests, BlockConfig, ResourceType};
use cloak_cdp::{launch, LaunchOptions};
use futures::StreamExt;
use std::time::Duration;

const URL: &str = "https://en.wikipedia.org/wiki/Web_scraping";

async fn transfer_bytes(block: bool) -> anyhow::Result<f64> {
    let mut opts = LaunchOptions::default();
    if block {
        opts.block_resources = vec![
            "image".into(),
            "stylesheet".into(),
            "font".into(),
            "media".into(),
        ];
    }
    let (mut browser, mut handler) = launch(opts).await?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser.new_page("about:blank").await?;
    let _guard = if block {
        Some(
            block_requests(
                &page,
                BlockConfig::new().resources([
                    ResourceType::Image,
                    ResourceType::Stylesheet,
                    ResourceType::Font,
                    ResourceType::Media,
                ]),
            )
            .await?,
        )
    } else {
        None
    };

    page.goto(URL).await?;
    tokio::time::sleep(Duration::from_secs(4)).await;

    // Sum transferred bytes via the Performance API (encoded body + headers).
    let bytes: f64 = page
        .evaluate(
            r#"performance.getEntriesByType('resource')
                 .reduce((n,e)=>n+(e.transferSize||0), 0)
               + (performance.getEntriesByType('navigation')[0]?.transferSize||0)"#,
        )
        .await?
        .into_value()?;

    browser.close().await?;
    handler_task.await?;
    Ok(bytes)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("measuring {URL}\n");

    let full = transfer_bytes(false).await?;
    println!("without blocking = {:>10.1} KB", full / 1024.0);

    let blocked = transfer_bytes(true).await?;
    println!("blocking img/css/font/media = {:>10.1} KB", blocked / 1024.0);

    if full > 0.0 {
        let saved = (1.0 - blocked / full) * 100.0;
        println!("\n=== saved {saved:.1}% of proxy bandwidth ===");
    }
    Ok(())
}
