//! Widevine CDM hint-seeding + fetcher. See docs/SPEC-cloakserve-widevine.md (PART 2).
//!
//! Two responsibilities:
//!  1. `seed_widevine_hint` / `resolve_widevine_cdm_dir` — pre-seed the Chromium
//!     Widevine CDM "hint file" into a persistent profile so a sideloaded CDM
//!     works on first launch (Linux-only; never downloads, never panics).
//!  2. `fetch_widevine_cdm` — download the CDM from Google's component-update
//!     server, verify it (CRX3 RSA publisher signature bound to the Widevine app
//!     id, and/or server SHA-256), and install it where resolution path 3 expects.
//!     Linux x86-64 only; returns errors (not process exit) elsewhere.
//!
//! Mirrors `cloakbrowser/widevine.py` (hint seeding) and
//! `bin/fetch-widevine.py` (fetcher).

use crate::error::{CloakError, Result};
use std::path::{Path, PathBuf};

/// Filename Chromium reads for the Widevine CDM hint.
const HINT_FILENAME: &str = "latest-component-updated-widevine-cdm";

/// Widevine CDM component id in Chromium's component updater.
const APP_ID: &str = "oimompecagnajdejgnnjijobebaeigek";
/// Google component-update JSON endpoint.
const UPDATE_URL: &str = "https://update.googleapis.com/service/update2/json";
/// Deliberately-low installed version so the server always reports an update.
const INSTALLED_VERSION: &str = "1.4.9.1088";
/// XSSI guard prefix the server prepends to JSON responses.
const XSSI_PREFIX: &str = ")]}'";

/// Whether hint seeding is disabled via the kill switch env var.
///
/// `CLOAKBROWSER_WIDEVINE` in {"0","false","off","no"} (trimmed, lowercased).
fn seeding_disabled() -> bool {
    match std::env::var("CLOAKBROWSER_WIDEVINE") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => false,
    }
}

/// A directory counts as a Widevine CDM dir only if it contains `manifest.json`.
fn dir_has_manifest(dir: &Path) -> bool {
    dir.join("manifest.json").is_file()
}

/// Resolve the Widevine CDM directory.
///
/// Resolution order (each candidate requires a `manifest.json` inside):
/// 1. `CLOAKBROWSER_WIDEVINE_CDM` env override — if set (non-empty/non-whitespace)
///    it is used *exclusively*: valid iff `<dir>/manifest.json` is a file, else
///    `None` (no fallthrough).
/// 2. `<dir of chrome binary>/WidevineCdm`.
/// 3. `<cache dir>/WidevineCdm` (`~/.cloakbrowser/WidevineCdm`).
///
/// The returned path is canonicalized (`.resolve()`d).
pub fn resolve_widevine_cdm_dir(binary_path: &Path) -> Option<PathBuf> {
    // 1. Env override — exclusive when set.
    match std::env::var("CLOAKBROWSER_WIDEVINE_CDM") {
        Ok(raw) if !raw.trim().is_empty() => {
            let custom = PathBuf::from(raw.trim());
            if dir_has_manifest(&custom) {
                return custom.canonicalize().ok();
            }
            return None;
        }
        _ => {}
    }

    // 2. Alongside the chrome binary.
    if let Some(bin_dir) = binary_path.parent() {
        let candidate = bin_dir.join("WidevineCdm");
        if dir_has_manifest(&candidate) {
            return candidate.canonicalize().ok();
        }
    }

    // 3. Cache dir.
    let candidate = crate::platform::cache_dir().join("WidevineCdm");
    if dir_has_manifest(&candidate) {
        return candidate.canonicalize().ok();
    }

    None
}

/// Hint file payload — `serde_json::to_string` produces the exact compact form
/// `{"Path":"<abs dir>"}` (no spaces).
#[derive(serde::Serialize)]
struct WidevineHint {
    #[serde(rename = "Path")]
    path: String,
}

/// Seed the Widevine hint file into a persistent profile.
///
/// No-op if: not Linux, seeding disabled via `CLOAKBROWSER_WIDEVINE`, or
/// `user_data_dir` is empty. Resolves the CDM dir; if `None`, logs (warn when
/// the env override was set-but-invalid, debug otherwise) and returns.
///
/// Writes `{"Path":"<abs cdm dir>"}` (compact, UTF-8) to
/// `<user_data_dir>/WidevineCdm/latest-component-updated-widevine-cdm`.
/// Idempotent (skips the write when content already matches) and **never
/// raises** — any I/O error is logged at warn level and swallowed.
pub fn seed_widevine_hint(user_data_dir: &Path, binary_path: &Path) -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    if seeding_disabled() {
        return Ok(());
    }
    if user_data_dir.as_os_str().is_empty() {
        return Ok(());
    }

    let cdm_dir = match resolve_widevine_cdm_dir(binary_path) {
        Some(d) => d,
        None => {
            let env_set = std::env::var("CLOAKBROWSER_WIDEVINE_CDM")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            if env_set {
                tracing::warn!(
                    "CLOAKBROWSER_WIDEVINE_CDM is set but no manifest.json was found; \
                     skipping Widevine hint seeding"
                );
            } else {
                tracing::debug!("no Widevine CDM directory found; skipping hint seeding");
            }
            return Ok(());
        }
    };

    let hint = WidevineHint {
        path: cdm_dir.to_string_lossy().into_owned(),
    };
    // serde_json produces compact JSON with no spaces: {"Path":"..."}.
    let content = match serde_json::to_string(&hint) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("failed to serialize Widevine hint: {e}");
            return Ok(());
        }
    };

    let hint_dir = user_data_dir.join("WidevineCdm");
    let hint_path = hint_dir.join(HINT_FILENAME);

    // Idempotent: skip when the file already holds the target content.
    if let Ok(existing) = std::fs::read_to_string(&hint_path) {
        if existing == content {
            return Ok(());
        }
    }

    // Best-effort — never raises.
    if let Err(e) = std::fs::create_dir_all(&hint_dir) {
        tracing::warn!(
            "failed to create Widevine hint dir {}: {e}",
            hint_dir.display()
        );
        return Ok(());
    }
    if let Err(e) = std::fs::write(&hint_path, content.as_bytes()) {
        tracing::warn!(
            "failed to write Widevine hint {}: {e}",
            hint_path.display()
        );
        return Ok(());
    }
    tracing::debug!("seeded Widevine hint → {}", hint_path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Fetcher — download + verify + install the Widevine CDM (Linux x86-64 only).
// ---------------------------------------------------------------------------

/// Map the host machine to the Widevine platform suffix (x86-64 only).
///
/// Google's component server publishes the Linux Widevine CDM for x86-64 only;
/// arm64/aarch64 are rejected with a clear message. Returns an error rather than
/// exiting the process.
fn arch_suffix() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x64"),
        "aarch64" | "arm" => Err(CloakError::Widevine(
            "the Widevine CDM is not published for linux arm64 (x86-64 only)".to_string(),
        )),
        other => Err(CloakError::Widevine(format!(
            "unsupported architecture for Widevine: {other:?}"
        ))),
    }
}

/// Read a base-128 varint from `b` starting at `i`; returns `(value, next_index)`.
fn read_varint(b: &[u8], mut i: usize) -> Result<(u64, usize)> {
    let mut shift = 0u32;
    let mut result: u64 = 0;
    loop {
        if i >= b.len() {
            return Err(CloakError::Verification("truncated varint".to_string()));
        }
        let byte = b[i];
        i += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok((result, i));
        }
        shift += 7;
        if shift > 63 {
            return Err(CloakError::Verification("varint too long".to_string()));
        }
    }
}

/// Minimal protobuf reader → `{field_num: [length-delimited bytes, ...]}`.
///
/// Handles wire types 0 (varint), 1 (64-bit), 2 (length-delimited), 5 (32-bit).
/// Only wire-type-2 fields retain their payloads; other wire types are skipped.
fn parse_pb(b: &[u8]) -> Result<std::collections::HashMap<u64, Vec<Vec<u8>>>> {
    let mut out: std::collections::HashMap<u64, Vec<Vec<u8>>> = std::collections::HashMap::new();
    let mut i = 0usize;
    let n = b.len();
    while i < n {
        let (tag, ni) = read_varint(b, i)?;
        i = ni;
        let field = tag >> 3;
        let wire = tag & 7;
        match wire {
            2 => {
                let (ln, ni) = read_varint(b, i)?;
                i = ni;
                let ln = ln as usize;
                if i + ln > b.len() {
                    return Err(CloakError::Verification(
                        "truncated length-delimited field".to_string(),
                    ));
                }
                out.entry(field).or_default().push(b[i..i + ln].to_vec());
                i += ln;
            }
            0 => {
                let (_, ni) = read_varint(b, i)?;
                i = ni;
            }
            1 => {
                i += 8;
            }
            5 => {
                i += 4;
            }
            other => {
                return Err(CloakError::Verification(format!(
                    "unsupported protobuf wire type {other}"
                )));
            }
        }
    }
    Ok(out)
}

/// CRX app id: first 16 bytes of SHA-256(pubkey_der), each nibble (high then low)
/// mapped to `chr(0x61 + nibble)` — i.e. the letters `a`–`p`.
///
/// Returns `(appid_string, digest16)`.
fn crx_appid(pubkey_der: &[u8]) -> (String, Vec<u8>) {
    use sha2::{Digest, Sha256};
    let full = Sha256::digest(pubkey_der);
    let digest16 = full[..16].to_vec();
    let mut s = String::with_capacity(32);
    for &byte in &digest16 {
        s.push((0x61 + (byte >> 4)) as char);
        s.push((0x61 + (byte & 0xF)) as char);
    }
    (s, digest16)
}

fn le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Verify the CRX3 RSA publisher signature and bind it to `APP_ID`.
///
/// Returns `Ok(true)` if a matching Widevine-publisher RSASSA-PKCS1-v1_5/SHA-256
/// proof verified. Returns a `Verification` error on a real failure (bad magic,
/// version mismatch, crx_id mismatch, invalid signature, or no matching proof).
///
/// Unlike the Python reference — where `cryptography` may be absent and the
/// function returns `False` to fall back to TLS+SHA-256 — the `rsa`/`sha2`
/// crates are always available here, so signature verification is always
/// attempted.
fn verify_crx3(crx_bytes: &[u8]) -> Result<bool> {
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use rsa::pkcs8::DecodePublicKey;
    use rsa::signature::Verifier;
    use rsa::RsaPublicKey;
    use sha2::Sha256;

    let verr = |m: &str| CloakError::Verification(m.to_string());

    if crx_bytes.len() < 12 {
        return Err(verr("not a CRX3 file (too short)"));
    }
    if &crx_bytes[..4] != b"Cr24" {
        return Err(verr("not a CRX3 file (bad magic)"));
    }
    let version = le_u32(&crx_bytes[4..8]);
    if version != 3 {
        return Err(CloakError::Verification(format!(
            "unexpected CRX version {version}"
        )));
    }
    let header_len = le_u32(&crx_bytes[8..12]) as usize;
    if 12 + header_len > crx_bytes.len() {
        return Err(verr("CRX3 header length out of bounds"));
    }
    let header = &crx_bytes[12..12 + header_len];
    let archive = &crx_bytes[12 + header_len..];

    let fields = parse_pb(header)?;
    let signed_header: &[u8] = fields
        .get(&10000)
        .and_then(|v| v.first())
        .map(|v| v.as_slice())
        .unwrap_or(b"");

    // Signed payload: "CRX3 SignedData\x00" + uint32LE(len) + signed_header + archive
    let mut payload = Vec::with_capacity(16 + 4 + signed_header.len() + archive.len());
    payload.extend_from_slice(b"CRX3 SignedData\x00");
    payload.extend_from_slice(&(signed_header.len() as u32).to_le_bytes());
    payload.extend_from_slice(signed_header);
    payload.extend_from_slice(archive);

    // SignedData.crx_id (field 1).
    let declared_id: Vec<u8> = if signed_header.is_empty() {
        Vec::new()
    } else {
        parse_pb(signed_header)?
            .get(&1)
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default()
    };

    // sha256_with_rsa proofs (field 2).
    for proof in fields.get(&2).map(|v| v.as_slice()).unwrap_or(&[]) {
        let p = parse_pb(proof)?;
        let pub_der = p.get(&1).and_then(|v| v.first());
        let sig = p.get(&2).and_then(|v| v.first());
        let (pub_der, sig) = match (pub_der, sig) {
            (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => (a, b),
            _ => continue,
        };

        let (appid, digest16) = crx_appid(pub_der);
        if appid != APP_ID {
            continue; // not the Widevine publisher key — ignore
        }
        if !declared_id.is_empty() && declared_id != digest16 {
            return Err(verr(
                "CRX signed-header crx_id does not match the signing key",
            ));
        }

        let public_key = RsaPublicKey::from_public_key_der(pub_der)
            .map_err(|e| CloakError::Verification(format!("bad CRX3 public key: {e}")))?;
        let verifying_key = VerifyingKey::<Sha256>::new(public_key);
        let signature = Signature::try_from(sig.as_slice())
            .map_err(|e| CloakError::Verification(format!("bad CRX3 signature bytes: {e}")))?;
        verifying_key
            .verify(&payload, &signature)
            .map_err(|_| verr("CRX3 publisher signature is INVALID"))?;
        return Ok(true);
    }
    Err(verr(
        "no CRX3 RSA proof from the expected Widevine publisher key",
    ))
}

/// Strip the XSSI `)]}'` prefix from a component-server JSON body, if present.
fn strip_xssi(body: &str) -> &str {
    body.strip_prefix(XSSI_PREFIX).unwrap_or(body)
}

/// Query the component server; return `(version, crx_url, sha256_hex)`.
async fn resolve_crx(
    client: &reqwest::Client,
    arch: &str,
) -> Result<(String, String, Option<String>)> {
    let payload = serde_json::json!({
        "request": {
            "@os": "",
            "@updater": "",
            "acceptformat": "crx3,download,puff,run,xz,zucc",
            "apps": [{
                "appid": APP_ID,
                "installsource": "ondemand",
                "updatecheck": {},
                "version": INSTALLED_VERSION,
            }],
            "dedup": "cr",
            "ismachine": false,
            "arch": arch,
            "os": { "arch": arch, "platform": "linux" },
            "protocol": "4.0",
            "updaterversion": "142.0.7444.175",
        }
    });

    let resp = client
        .post(UPDATE_URL)
        .header("User-Agent", "Mozilla/5.0")
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(&payload).map_err(|e| CloakError::other(e.to_string()))?)
        .send()
        .await?;
    let body = resp.text().await?;
    let json: serde_json::Value = serde_json::from_str(strip_xssi(&body))
        .map_err(|e| CloakError::Widevine(format!("component server sent invalid JSON: {e}")))?;

    let uc = json
        .get("response")
        .and_then(|v| v.get("apps"))
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("updatecheck"))
        .ok_or_else(|| {
            CloakError::Widevine("component server response missing updatecheck".to_string())
        })?;

    if let Some(status) = uc.get("status").and_then(|v| v.as_str()) {
        if status != "ok" {
            return Err(CloakError::Widevine(format!(
                "component server returned status={status:?} (no update available)"
            )));
        }
    }

    let version = uc
        .get("nextversion")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();

    // Find the first operation that carries download URLs + its sha256.
    if let Some(pipelines) = uc.get("pipelines").and_then(|v| v.as_array()) {
        for pipeline in pipelines {
            let ops = match pipeline.get("operations").and_then(|v| v.as_array()) {
                Some(o) => o,
                None => continue,
            };
            for op in ops {
                let url = op
                    .get("urls")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| {
                        arr.iter()
                            .filter_map(|u| u.get("url").and_then(|x| x.as_str()))
                            .find(|s| s.starts_with("https"))
                    });
                if let Some(url) = url {
                    let sha = op
                        .get("out")
                        .and_then(|v| v.get("sha256"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    return Ok((version, url.to_string(), sha));
                }
            }
        }
    }
    Err(CloakError::Widevine(
        "no CRX download URL in component server response".to_string(),
    ))
}

/// Download the CRX blob; when `sha256_hex` is present, verify it (over TLS).
async fn download_crx(
    client: &reqwest::Client,
    url: &str,
    sha256_hex: Option<&str>,
) -> Result<Vec<u8>> {
    let resp = client.get(url).send().await?;
    let blob = resp.bytes().await?.to_vec();
    if let Some(expected) = sha256_hex {
        use sha2::{Digest, Sha256};
        let got = Sha256::digest(&blob);
        let got_hex = hex_lower(&got);
        if got_hex != expected.to_ascii_lowercase() {
            return Err(CloakError::Verification(format!(
                "sha256 mismatch: expected {expected}, got {got_hex}"
            )));
        }
    }
    Ok(blob)
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Extract `manifest.json` + `libwidevinecdm.so` from a CRX (opened as a zip)
/// into a temp dir alongside `out_dir`, then atomically swap into place.
fn extract_crx(crx_bytes: &[u8], arch: &str, out_dir: &Path) -> Result<()> {
    use std::io::Read;
    let so_member = format!("_platform_specific/linux_{arch}/libwidevinecdm.so");

    // zip locates the central directory from the end of the file, so a CRX3
    // (CRX header + embedded zip) opens directly — the header is ignored.
    let cursor = std::io::Cursor::new(crx_bytes);
    let mut zf = zip::ZipArchive::new(cursor)
        .map_err(|e| CloakError::Extraction(format!("failed to open CRX as zip: {e}")))?;

    let names: std::collections::HashSet<String> =
        zf.file_names().map(|s| s.to_string()).collect();
    if !names.contains("manifest.json") || !names.contains(&so_member) {
        return Err(CloakError::Extraction(format!(
            "CRX missing expected members (manifest.json / {so_member})"
        )));
    }

    // Stage in a temp dir next to out_dir; the rename swap is atomic.
    let parent = out_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&parent)?;

    let tmp = tempfile::Builder::new()
        .prefix(".widevine.tmp.")
        .tempdir_in(&parent)?;
    let tmp_path = tmp.path().to_path_buf();

    // Extract manifest.json.
    {
        let mut f = zf
            .by_name("manifest.json")
            .map_err(|e| CloakError::Extraction(format!("manifest.json: {e}")))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        std::fs::write(tmp_path.join("manifest.json"), &buf)?;
    }

    // Extract the .so into its nested path.
    {
        let mut f = zf
            .by_name(&so_member)
            .map_err(|e| CloakError::Extraction(format!("{so_member}: {e}")))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        let so_dest = tmp_path.join(&so_member);
        if let Some(so_parent) = so_dest.parent() {
            std::fs::create_dir_all(so_parent)?;
        }
        std::fs::write(&so_dest, &buf)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&so_dest, std::fs::Permissions::from_mode(0o644))?;
        }
    }

    // Swap into place. Persist the temp dir (so Drop won't delete it), then
    // remove any existing out_dir and rename. The rename is atomic; the
    // preceding rmtree is not.
    let staged = tmp.keep();
    let cleanup = |e: CloakError| -> CloakError {
        let _ = std::fs::remove_dir_all(&staged);
        e
    };
    if out_dir.exists() {
        std::fs::remove_dir_all(out_dir).map_err(|e| cleanup(CloakError::Io(e)))?;
    }
    if let Err(e) = std::fs::rename(&staged, out_dir) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(CloakError::Extraction(format!(
            "failed to move Widevine CDM into place: {e}"
        )));
    }
    Ok(())
}

/// Download, verify, and install the Widevine CDM into `out`.
///
/// Layout produced:
///   `<out>/manifest.json`
///   `<out>/_platform_specific/linux_<arch>/libwidevinecdm.so`
///
/// Linux x86-64 only — returns a `Widevine` error (not a process exit) on any
/// other OS/arch. When `force` is false and `<out>/manifest.json` already
/// exists, this is a cache hit and returns `out` unchanged.
///
/// Integrity: requires **at least one** positive check before installing the
/// native `.so` — the CRX3 RSA publisher signature (bound to the Widevine app
/// id) and/or the server-provided SHA-256 over TLS. If neither is available it
/// refuses. Returns the canonical output directory on success.
pub async fn fetch_widevine_cdm(out: &Path, force: bool) -> Result<PathBuf> {
    // Linux only: the hint-file mechanism is Linux/ChromeOS-specific and the
    // .so we fetch is a Linux binary.
    if !cfg!(target_os = "linux") {
        return Err(CloakError::Widevine(format!(
            "Widevine fetch is Linux-only (this host is {})",
            std::env::consts::OS
        )));
    }

    let out = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()?.join(out)
    };

    // Cache hit.
    if out.join("manifest.json").is_file() && !force {
        tracing::info!("Widevine CDM already present (cache hit): {}", out.display());
        return Ok(out);
    }

    let arch = arch_suffix()?;
    tracing::info!("querying component server (linux {arch})…");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    let (version, url, sha) = resolve_crx(&client, arch).await?;
    tracing::info!("Widevine CDM {version} → downloading…");
    let blob = download_crx(&client, &url, sha.as_deref()).await?;

    // Integrity policy: require >=1 positive check. The CRX3 publisher signature
    // is the primary guarantee; the server SHA-256 over TLS is the fallback.
    let sig_ok = verify_crx3(&blob)?;
    match (sig_ok, sha.is_some()) {
        (true, true) => tracing::info!(
            "verified {} bytes (SHA-256 + CRX3 publisher signature)",
            blob.len()
        ),
        (true, false) => tracing::info!(
            "verified {} bytes (CRX3 publisher signature; server sent no SHA-256)",
            blob.len()
        ),
        (false, true) => tracing::info!(
            "verified {} bytes (SHA-256 over TLS; CRX3 sig not present)",
            blob.len()
        ),
        (false, false) => {
            return Err(CloakError::Verification(
                "refusing to install: server provided no SHA-256 and no CRX3 publisher \
                 signature could be verified — cannot confirm CDM integrity"
                    .to_string(),
            ));
        }
    }

    tracing::info!("extracting → {}", out.display());
    extract_crx(&blob, arch, &out)?;
    tracing::info!("done");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crx_appid_nibble_encoding() {
        // appid = first 16 bytes of SHA-256(pubkey), each byte → high nibble
        // then low nibble, mapped a..p.
        use sha2::{Digest, Sha256};
        let pubkey = b"hello widevine";
        let digest = Sha256::digest(pubkey);
        let (appid, digest16) = crx_appid(pubkey);

        assert_eq!(digest16.as_slice(), &digest[..16]);
        assert_eq!(appid.len(), 32);
        // Rebuild expected string independently.
        let mut expected = String::new();
        for &b in &digest[..16] {
            expected.push((0x61 + (b >> 4)) as char);
            expected.push((0x61 + (b & 0xF)) as char);
        }
        assert_eq!(appid, expected);
        // Every char is within a..p.
        assert!(appid.chars().all(|c| ('a'..='p').contains(&c)));
    }

    #[test]
    fn test_crx_appid_known_nibbles() {
        // Boundary byte values → nibble pairs.
        let map = |b: u8| -> String {
            let mut s = String::new();
            s.push((0x61 + (b >> 4)) as char);
            s.push((0x61 + (b & 0xF)) as char);
            s
        };
        assert_eq!(map(0x00), "aa");
        assert_eq!(map(0x0f), "ap");
        assert_eq!(map(0xf0), "pa");
        assert_eq!(map(0xff), "pp");
        assert_eq!(map(0x1a), "bk");
    }

    #[test]
    fn test_strip_xssi() {
        assert_eq!(strip_xssi(")]}'{\"a\":1}"), "{\"a\":1}");
        // No prefix → unchanged.
        assert_eq!(strip_xssi("{\"a\":1}"), "{\"a\":1}");
        // Prefix followed by newline (real responses).
        assert_eq!(strip_xssi(")]}'\n{\"a\":1}"), "\n{\"a\":1}");
        // Empty stays empty.
        assert_eq!(strip_xssi(""), "");
    }

    #[test]
    fn test_hint_json_compact() {
        let hint = WidevineHint {
            path: "/home/user/.cloakbrowser/WidevineCdm".to_string(),
        };
        let s = serde_json::to_string(&hint).unwrap();
        assert_eq!(s, r#"{"Path":"/home/user/.cloakbrowser/WidevineCdm"}"#);
    }
}
