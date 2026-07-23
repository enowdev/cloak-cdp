//! Proxy config + WebRTC arg resolution. See docs/SPEC-geoip-proxy.md (PART B).
//!
//! STUB — to be implemented.

use crate::Proxy;

/// Resolved proxy configuration: chromiumoxide-level settings + extra chrome args.
#[derive(Debug, Default, Clone)]
pub struct ResolvedProxy {
    /// Proxy dict for the launcher (server/username/password/bypass), if any.
    pub proxy_settings: Option<crate::ProxySettings>,
    /// Extra `--proxy-server` / `--proxy-bypass-list` args, if any.
    pub extra_args: Vec<String>,
}

/// Merge geoip results with explicit timezone/locale. Returns `(tz, locale, exit_ip)`.
pub async fn maybe_resolve_geoip(
    _geoip: bool,
    _proxy: Option<&Proxy>,
    _timezone: Option<String>,
    _locale: Option<String>,
    _args: Option<&[String]>,
) -> (Option<String>, Option<String>, Option<String>) {
    unimplemented!("proxy::maybe_resolve_geoip — see docs/SPEC-geoip-proxy.md")
}

/// Split proxy into launcher settings vs chrome flags.
pub fn resolve_proxy_config(_proxy: Option<&Proxy>) -> ResolvedProxy {
    unimplemented!("proxy::resolve_proxy_config — see docs/SPEC-geoip-proxy.md")
}

/// Replace `--fingerprint-webrtc-ip=auto` with the resolved exit IP.
pub async fn resolve_webrtc_args(
    _args: Option<Vec<String>>,
    _proxy: Option<&Proxy>,
) -> Option<Vec<String>> {
    unimplemented!("proxy::resolve_webrtc_args — see docs/SPEC-geoip-proxy.md")
}

/// Append `--fingerprint-webrtc-ip=<exit_ip>` unless already present.
pub fn append_webrtc_exit_ip(
    _args: Option<Vec<String>>,
    _exit_ip: Option<&str>,
) -> Option<Vec<String>> {
    unimplemented!("proxy::append_webrtc_exit_ip — see docs/SPEC-geoip-proxy.md")
}
