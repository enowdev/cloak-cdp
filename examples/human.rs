//! Humanize layer end-to-end: drive a real page with human-like mouse + typing.
//!
//! Proves the pure behavioral core (bezier mouse, typing cadence) is wired to
//! chromiumoxide via the CDP `RawMouse`/`RawKeyboard` driver.
//!
//! Run with: `cargo run --example human`

use cloakbrowser::human::{human_move, human_type, resolve_config, CdpKeyboard, CdpMouse};
use cloakbrowser::{launch, LaunchOptions};
use futures::StreamExt;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let headless = std::env::var("HEADED").is_err();
    println!("launching (headless={headless}) ...");
    let (mut browser, mut handler) = launch(LaunchOptions {
        headless,
        ..Default::default()
    })
    .await?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser.new_page("about:blank").await?;
    // A trivial page with a text input and a mousemove/keypress log.
    page.goto("data:text/html,<input id=t style='font-size:24px;margin:80px'>\
        <div id=log></div><script>\
        let m=0;addEventListener('mousemove',()=>m++);\
        t.addEventListener('input',()=>log.textContent='typed: '+t.value);\
        </script>")
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let cfg = resolve_config("default", None)?;
    let mouse = CdpMouse { page: &page };
    let kb = CdpKeyboard { page: &page };
    // human_type/move require a Send rng (the futures are boxed); ThreadRng is
    // !Send, so use a seedable StdRng.
    use rand::SeedableRng;
    let mut rng = rand::rngs::StdRng::from_entropy();

    // Human mouse move from (100,100) to the input around (200,96).
    println!("human mouse move ...");
    human_move(&mouse, 100.0, 100.0, 200.0, 96.0, &cfg, &mut rng).await;

    // Focus the input, then human-type into it.
    page.evaluate("document.getElementById('t').focus()").await?;
    println!("human typing ...");
    human_type(&kb, "Hello from Rust!", &cfg, &mut rng).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let typed: String = page
        .evaluate("document.getElementById('t').value")
        .await?
        .into_value()?;
    let moves: i64 = page.evaluate("m").await?.into_value()?;

    println!("\n=== result ===");
    println!("input value = {typed:?}");
    println!("mousemove events fired = {moves}  (bezier path => many events)");
    println!(
        "verdict: {}",
        if typed == "Hello from Rust!" && moves > 5 {
            "OK — human mouse + typing wired through CDP"
        } else {
            "MISMATCH"
        }
    );

    browser.close().await?;
    handler_task.await?;
    Ok(())
}
