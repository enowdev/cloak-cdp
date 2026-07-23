//! GeoIP resolution (timezone/locale/exit-IP) via MaxMind + IP echo services.
//! See docs/SPEC-geoip-proxy.md (PART A).
//!
//! STUB — to be implemented.

/// Resolve `(timezone, locale)` from a proxy (or the machine's egress).
pub async fn resolve_proxy_geo(_proxy_url: Option<&str>) -> (Option<String>, Option<String>) {
    unimplemented!("geoip::resolve_proxy_geo — see docs/SPEC-geoip-proxy.md")
}

/// Resolve `(timezone, locale, exit_ip)`.
pub async fn resolve_proxy_geo_with_ip(
    _proxy_url: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    unimplemented!("geoip::resolve_proxy_geo_with_ip — see docs/SPEC-geoip-proxy.md")
}

/// Resolve just the egress IP (for WebRTC spoofing).
pub async fn resolve_proxy_exit_ip(_proxy_url: Option<&str>) -> Option<String> {
    unimplemented!("geoip::resolve_proxy_exit_ip — see docs/SPEC-geoip-proxy.md")
}
