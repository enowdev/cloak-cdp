//! Connect to a running cloakserve multiplexer over CDP and drive a page.
//!
//! Start the server first:
//!   cargo run --example serve --features serve   (PORT=9223)
//! Then:
//!   MUX=http://127.0.0.1:9223?fingerprint=12345 cargo run --example serve_client

use chromiumoxide::Browser;
use futures::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mux = std::env::var("MUX")
        .unwrap_or_else(|_| "http://127.0.0.1:9223?fingerprint=12345".to_string());
    println!("multiplexer: {mux}");

    // CDP handshake: GET /json/version to get the (rewritten) WebSocket URL,
    // preserving the query string (that's what selects/spawns the seed).
    let (base, query) = mux.split_once('?').unwrap_or((mux.as_str(), ""));
    let version_url = format!("{}/json/version?{}", base.trim_end_matches('/'), query);
    let body = reqwest::get(&version_url).await?.text().await?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    let ws = v["webSocketDebuggerUrl"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no webSocketDebuggerUrl"))?;
    println!("resolved WS = {ws}");

    let (browser, mut handler) = Browser::connect(ws).await?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser.new_page("about:blank").await?;
    let ua: String = page.evaluate("navigator.userAgent").await?.into_value()?;
    let wd: serde_json::Value = page.evaluate("navigator.webdriver").await?.into_value()?;
    println!("navigator.webdriver = {wd}");
    println!("userAgent = {ua}");

    println!(
        "verdict: {}",
        if wd == serde_json::Value::Bool(false) {
            "OK — controlled a page through the cloakserve multiplexer, stealth intact"
        } else {
            "webdriver leaked"
        }
    );

    // Don't close the shared browser (other seeds may use it); just drop.
    let _ = browser;
    handler_task.abort();
    Ok(())
}
