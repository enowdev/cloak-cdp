//! Live test: GeoIP resolution (downloads ~70MB GeoLite2-City.mmdb on first run
//! into CLOAKBROWSER_CACHE_DIR/geoip), then resolves timezone/locale/exit-IP.
//!
//! Optionally set PROXY_URL to resolve through a proxy.
//!
//! Run: CLOAKBROWSER_CACHE_DIR=/tmp/cloak-geo cargo run --example verify_geoip

use cloak_cdp::geoip::{resolve_proxy_exit_ip, resolve_proxy_geo_with_ip};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let proxy = std::env::var("PROXY_URL").ok();
    println!("proxy = {proxy:?}  (None => resolve this machine's egress)");

    println!("resolving exit IP ...");
    let ip = resolve_proxy_exit_ip(proxy.as_deref()).await;
    println!("exit_ip = {ip:?}");

    println!("resolving geo (tz/locale via MaxMind, may download ~70MB DB) ...");
    let t0 = std::time::Instant::now();
    let (tz, locale, ip2) = resolve_proxy_geo_with_ip(proxy.as_deref()).await;
    println!("elapsed = {:.1}s", t0.elapsed().as_secs_f64());

    println!("\n=== result ===");
    println!("timezone = {tz:?}");
    println!("locale   = {locale:?}");
    println!("exit_ip  = {ip2:?}");

    let ok = ip.is_some() && tz.is_some();
    println!(
        "\nverdict: {}",
        if ok {
            "OK — egress IP + timezone resolved"
        } else {
            "PARTIAL (see values above)"
        }
    );
    Ok(())
}
