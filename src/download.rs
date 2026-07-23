//! Binary download + Ed25519 verification + extraction. See docs/SPEC-download.md.
//!
//! Free public path only (no login/license). Mirrors `cloakbrowser/download.py`:
//! download primary then GitHub fallback (skip fallback when a custom
//! `CLOAKBROWSER_DOWNLOAD_URL` is set), verify (official Ed25519 signed
//! SHA256SUMS, or custom-URL checksum-only), then extract (tar.gz or zip) with
//! symlink/traversal guards, single-subdir flatten, executable bit, and macOS
//! quarantine removal.

use crate::error::{CloakError, Result};
use crate::platform;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// Streaming read/write chunk size (matches Python's 8192).
const CHUNK_SIZE: usize = 8192;
/// Manifest fetch timeout (SHA256SUMS + .sig).
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Ensure the stealth Chromium binary is present, downloading + verifying if
/// needed. Returns the path to the chrome executable.
pub async fn ensure_binary(browser_version: Option<&str>) -> Result<PathBuf> {
    // 1. Local override — skip all download/verify/extract.
    if let Ok(override_path) = std::env::var("CLOAKBROWSER_BINARY_PATH") {
        if !override_path.is_empty() {
            let path = PathBuf::from(&override_path);
            if !path.exists() {
                return Err(CloakError::BinaryNotFound(format!(
                    "CLOAKBROWSER_BINARY_PATH set to '{override_path}' but file does not exist"
                )));
            }
            tracing::info!("Using local binary override: {}", override_path);
            return Ok(path);
        }
    }

    // 2. Resolve requested version pin (arg or CLOAKBROWSER_VERSION).
    let requested_version = platform::normalize_requested_version(browser_version)?;

    // 4. Fail fast if this platform has no downloadable binary.
    platform::check_platform_available()?;

    // 5. Pinned path.
    if let Some(ref requested) = requested_version {
        let binary_path = platform::binary_path(Some(requested));
        if binary_path.exists() && is_executable(&binary_path) {
            tracing::debug!(
                "Pinned binary found in cache: {} (version {})",
                binary_path.display(),
                requested
            );
            return Ok(binary_path);
        }

        tracing::info!(
            "Stealth Chromium {} not found. Downloading for {}...",
            requested,
            platform::platform_tag()?
        );
        download_and_extract(Some(requested)).await?;

        if !(binary_path.exists() && is_executable(&binary_path)) {
            return Err(CloakError::BinaryNotFound(format!(
                "Pinned download completed but binary not found at expected path: {}",
                binary_path.display()
            )));
        }
        return Ok(binary_path);
    }

    // 6. Unpinned path: prefer auto-updated marker version, else hardcoded.
    let effective = effective_version();
    let binary_path = platform::binary_path(Some(&effective));
    if binary_path.exists() && is_executable(&binary_path) {
        tracing::debug!(
            "Binary found in cache: {} (version {})",
            binary_path.display(),
            effective
        );
        return Ok(binary_path);
    }

    // Fall back to the platform's hardcoded version if the effective differs.
    let platform_version = platform::chromium_version();
    if effective != platform_version {
        let fallback_path = platform::binary_path(None);
        if fallback_path.exists() && is_executable(&fallback_path) {
            tracing::debug!("Binary found in cache: {}", fallback_path.display());
            return Ok(fallback_path);
        }
    }

    tracing::info!(
        "Stealth Chromium {} not found. Downloading for {}...",
        platform_version,
        platform::platform_tag()?
    );
    download_and_extract(None).await?;

    let binary_path = platform::binary_path(None);
    if !binary_path.exists() {
        return Err(CloakError::BinaryNotFound(format!(
            "Download completed but binary not found at expected path: {}",
            binary_path.display()
        )));
    }
    Ok(binary_path)
}

// ---------------------------------------------------------------------------
// Version resolution (marker-aware, no periodic update machinery)
// ---------------------------------------------------------------------------

/// Free-tier effective version: hardcoded base unless a cache marker names a
/// newer, already-downloaded version. Reads `latest_version_{tag}` then legacy
/// `latest_version`.
fn effective_version() -> String {
    let base = platform::chromium_version();
    let cache = platform::cache_dir();

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(tag) = platform::platform_tag() {
        candidates.push(cache.join(format!("latest_version_{tag}")));
    }
    candidates.push(cache.join("latest_version"));

    for marker in candidates {
        let Ok(contents) = std::fs::read_to_string(&marker) else {
            continue;
        };
        let version = contents.trim();
        if version.is_empty() {
            continue;
        }
        if platform::version_newer(version, &base)
            && platform::binary_path(Some(version)).exists()
        {
            return version.to_string();
        }
    }
    base
}

// ---------------------------------------------------------------------------
// Download orchestration
// ---------------------------------------------------------------------------

async fn download_and_extract(version: Option<&str>) -> Result<()> {
    let v = version
        .map(|s| s.to_string())
        .unwrap_or_else(platform::chromium_version);
    let archive_name = platform::archive_name(platform::platform_tag()?);

    let primary_url = format!(
        "{}/chromium-v{}/{}",
        platform::download_base_url(),
        v,
        archive_name
    );
    let fallback_url = format!(
        "{}/chromium-v{}/{}",
        platform::GITHUB_DOWNLOAD_BASE_URL,
        v,
        archive_name
    );

    let binary_dir = platform::binary_dir(version);
    let binary_path = platform::binary_path(version);

    if let Some(parent) = binary_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Download to a temp file first (atomic — no partial downloads in cache).
    let tmp = tempfile::Builder::new()
        .suffix(platform::archive_ext())
        .tempfile()?;
    let tmp_path = tmp.path().to_path_buf();

    let result = async {
        // Primary, then GitHub fallback (skip fallback if custom URL).
        match download_file(&primary_url, &tmp_path).await {
            Ok(()) => {}
            Err(primary_err) => {
                if platform::has_custom_download_url() {
                    return Err(primary_err);
                }
                tracing::warn!(
                    "Primary download failed ({}), trying GitHub Releases...",
                    primary_err
                );
                download_file(&fallback_url, &tmp_path).await?;
            }
        }

        verify_download(&tmp_path, version).await?;
        extract_archive(&tmp_path, &binary_dir, &binary_path)?;
        Ok(())
    }
    .await;

    // tmp file is removed when `tmp` drops.
    drop(tmp);
    result
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::default())
        .build()
        .map_err(CloakError::from)
}

async fn download_file(url: &str, dest: &Path) -> Result<()> {
    tracing::info!("Downloading from {}", url);

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::default())
        .build()?;

    let resp = client.get(url).send().await?.error_for_status()?;

    let mut file = tokio::fs::File::create(dest).await?;
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        // Write in CHUNK_SIZE slices to mirror the 8192-byte streaming loop.
        for slice in chunk.chunks(CHUNK_SIZE) {
            file.write_all(slice).await?;
        }
        downloaded += chunk.len() as u64;
    }
    file.flush().await?;
    tracing::info!("Download complete: {} MB", downloaded / (1024 * 1024));
    Ok(())
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

async fn verify_download(file_path: &Path, version: Option<&str>) -> Result<()> {
    let tarball_name = platform::archive_name(platform::platform_tag()?);

    if platform::has_custom_download_url() {
        // Self-hosted mirror: signature scheme does not apply. Legacy
        // same-origin checksum, skippable via CLOAKBROWSER_SKIP_CHECKSUM.
        let skip = std::env::var("CLOAKBROWSER_SKIP_CHECKSUM")
            .map(|s| s.to_lowercase() == "true")
            .unwrap_or(false);
        if skip {
            tracing::warn!(
                "CLOAKBROWSER_SKIP_CHECKSUM set — skipping verification for custom download URL"
            );
            return Ok(());
        }
        let Some(checksums) = fetch_checksums(version).await else {
            tracing::warn!(
                "SHA256SUMS not available from custom URL — skipping checksum verification"
            );
            return Ok(());
        };
        let Some(expected) = checksums.get(&tarball_name) else {
            tracing::warn!(
                "SHA256SUMS found but no entry for {} — skipping verification",
                tarball_name
            );
            return Ok(());
        };
        verify_checksum(file_path, expected)?;
        return Ok(());
    }

    // Official path: Ed25519 signature is the trust root and non-bypassable.
    let Some((manifest_bytes, sig_bytes)) = fetch_signed_manifest(version).await else {
        return Err(CloakError::Verification(
            "Could not fetch a signed SHA256SUMS (SHA256SUMS + SHA256SUMS.sig) for this \
             release — refusing to use an unverified binary. Retry, or report at \
             https://github.com/CloakHQ/cloakbrowser/issues"
                .to_string(),
        ));
    };

    verify_signature(&manifest_bytes, &sig_bytes)?;
    let manifest_text = String::from_utf8_lossy(&manifest_bytes).into_owned();

    // Version binding: the signed manifest must declare the requested version.
    let requested = version
        .map(|s| s.to_string())
        .unwrap_or_else(platform::chromium_version);
    let declared = parse_manifest_version(&manifest_text);
    if declared.as_deref() != Some(requested.as_str()) {
        return Err(CloakError::Verification(format!(
            "Version mismatch in signed SHA256SUMS: requested {requested}, manifest declares \
             {}. Refusing (possible downgrade).",
            declared.as_deref().unwrap_or("none")
        )));
    }

    let checksums = parse_checksums(&manifest_text);
    let Some(expected) = checksums.get(&tarball_name) else {
        return Err(CloakError::Verification(format!(
            "Signature-verified SHA256SUMS has no entry for {tarball_name} — cannot confirm \
             binary integrity."
        )));
    };
    verify_checksum(file_path, expected)?;
    Ok(())
}

/// Fetch (SHA256SUMS, SHA256SUMS.sig) raw bytes, primary origin then GitHub
/// mirror. Both files come from the SAME origin per iteration. `None` if all fail.
async fn fetch_signed_manifest(version: Option<&str>) -> Option<(Vec<u8>, Vec<u8>)> {
    let v = version
        .map(|s| s.to_string())
        .unwrap_or_else(platform::chromium_version);
    let bases = [
        format!("{}/chromium-v{}", platform::download_base_url(), v),
        format!("{}/chromium-v{}", platform::GITHUB_DOWNLOAD_BASE_URL, v),
    ];
    let client = http_client().ok()?;
    for base in bases {
        match fetch_manifest_pair(&client, &base).await {
            Ok(pair) => return Some(pair),
            Err(_) => continue,
        }
    }
    None
}

async fn fetch_manifest_pair(client: &reqwest::Client, base: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let manifest = client
        .get(format!("{base}/SHA256SUMS"))
        .timeout(MANIFEST_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let sig = client
        .get(format!("{base}/SHA256SUMS.sig"))
        .timeout(MANIFEST_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    Ok((manifest.to_vec(), sig.to_vec()))
}

/// Fetch and parse SHA256SUMS (custom-URL checksum-only path). Respects the
/// custom-URL contract (no GitHub fallback when a custom URL is set).
async fn fetch_checksums(version: Option<&str>) -> Option<HashMap<String, String>> {
    let v = version
        .map(|s| s.to_string())
        .unwrap_or_else(platform::chromium_version);
    let mut urls = vec![format!(
        "{}/chromium-v{}/SHA256SUMS",
        platform::download_base_url(),
        v
    )];
    if !platform::has_custom_download_url() {
        urls.push(format!(
            "{}/chromium-v{}/SHA256SUMS",
            platform::GITHUB_DOWNLOAD_BASE_URL,
            v
        ));
    }
    let client = http_client().ok()?;
    for url in urls {
        let fetched = async {
            let text = client
                .get(&url)
                .timeout(MANIFEST_TIMEOUT)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            Ok::<_, reqwest::Error>(text)
        }
        .await;
        if let Ok(text) = fetched {
            return Some(parse_checksums(&text));
        }
    }
    None
}

/// Verify a detached Ed25519 signature over the raw manifest bytes. `sig_b64`
/// is base64 of the 64-byte raw signature. Tries each pinned key; succeeds if
/// any validates. Fails closed.
fn verify_signature(manifest_bytes: &[u8], sig_b64: &[u8]) -> Result<()> {
    // Strip surrounding ASCII whitespace, matching Python's `.strip()`.
    let trimmed: &[u8] = {
        let start = sig_b64
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(sig_b64.len());
        let end = sig_b64
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map(|i| i + 1)
            .unwrap_or(0);
        if start >= end {
            &[]
        } else {
            &sig_b64[start..end]
        }
    };

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .map_err(|e| {
            CloakError::Verification(format!("Malformed SHA256SUMS.sig (not valid base64): {e}"))
        })?;

    let sig_arr: [u8; 64] = sig_bytes.as_slice().try_into().map_err(|_| {
        CloakError::Verification(format!(
            "Malformed SHA256SUMS.sig (expected 64-byte signature, got {})",
            sig_bytes.len()
        ))
    })?;
    let signature = Signature::from_bytes(&sig_arr);

    for pubkey_b64 in platform::BINARY_SIGNING_PUBKEYS {
        // Skip an unparsable pinned key rather than aborting.
        let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(pubkey_b64) else {
            continue;
        };
        let raw_arr: [u8; 32] = match raw.as_slice().try_into() {
            Ok(a) => a,
            Err(_) => continue,
        };
        let Ok(key) = VerifyingKey::from_bytes(&raw_arr) else {
            continue;
        };
        if key.verify_strict(manifest_bytes, &signature).is_ok() {
            tracing::info!("SHA256SUMS signature verified: Ed25519 OK");
            return Ok(());
        }
    }

    Err(CloakError::Verification(
        "SHA256SUMS signature verification failed — no pinned key validated the manifest. \
         The binary's authenticity could not be confirmed. Report at \
         https://github.com/CloakHQ/cloakbrowser/issues"
            .to_string(),
    ))
}

/// Read the `version=<v>` line from a signed manifest. `None` if absent.
fn parse_manifest_version(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("version=") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Parse SHA256SUMS format: `<64-hex sha256><whitespace><filename>` per line.
/// Only lines whose first token is a 64-char hex digest are accepted; the key
/// is the filename with any leading `*` stripped.
fn parse_checksums(text: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for line in text.trim().lines() {
        let line = line.trim();
        // split(None, 1): first token, then the remainder as filename.
        let mut it = line.splitn(2, char::is_whitespace);
        let (Some(hash_val), Some(filename)) = (it.next(), it.next()) else {
            continue;
        };
        // The remainder may have leading whitespace (Python's split(None, 1)
        // consumes all leading whitespace of the second field).
        let filename = filename.trim_start();
        if filename.is_empty() {
            continue;
        }
        let hash_val = hash_val.to_lowercase();
        // hash_val lowercased, so is_ascii_hexdigit only matches 0-9a-f here.
        if hash_val.len() != 64 || !hash_val.bytes().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        result.insert(filename.trim_start_matches('*').to_string(), hash_val);
    }
    result
}

/// Verify SHA-256 of a file (streamed 8192-byte chunks). Error on mismatch.
fn verify_checksum(file_path: &Path, expected_hash: &str) -> Result<()> {
    let mut file = std::fs::File::open(file_path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; CHUNK_SIZE];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hex_lower(&hasher.finalize());
    if actual != expected_hash {
        return Err(CloakError::Verification(format!(
            "Checksum verification failed!\n  Expected: {expected_hash}\n  Got:      {actual}\n  \
             File may be corrupted or tampered with. Please retry or report at \
             https://github.com/CloakHQ/cloakbrowser/issues"
        )));
    }
    tracing::info!("Checksum verified: SHA-256 OK");
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

fn extract_archive(archive_path: &Path, dest_dir: &Path, binary_path: &Path) -> Result<()> {
    tracing::info!("Extracting to {}", dest_dir.display());

    // Clean existing dir if a partial download existed.
    if dest_dir.exists() {
        std::fs::remove_dir_all(dest_dir)?;
    }
    std::fs::create_dir_all(dest_dir)?;

    let is_zip = archive_path.to_string_lossy().ends_with(".zip");
    if is_zip {
        extract_zip(archive_path, dest_dir)?;
    } else {
        extract_tar(archive_path, dest_dir)?;
    }

    // If extraction created a single non-.app subdirectory, flatten it.
    flatten_single_subdir(dest_dir)?;

    // Make the binary executable.
    if binary_path.exists() {
        make_executable(binary_path)?;
    }

    // macOS: remove quarantine/provenance xattrs so Gatekeeper doesn't prompt.
    #[cfg(target_os = "macos")]
    remove_quarantine(dest_dir);

    if binary_path.exists() {
        tracing::info!("Binary ready: {}", binary_path.display());
    }
    Ok(())
}

/// A resolved-path traversal check mirroring Python's
/// `str(resolved).startswith(str(dest.resolve()))`. Uses lexical normalization
/// (the entry does not yet exist, so `canonicalize` cannot be used on it).
fn is_within(dest_resolved: &Path, candidate: &Path) -> bool {
    let normalized = lexical_normalize(candidate);
    normalized.starts_with(dest_resolved)
}

/// Lexically normalize a path (resolve `.` and `..` components without touching
/// the filesystem). Used for traversal checks on entries not yet on disk.
fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn extract_tar(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let dest_resolved = dest_dir
        .canonicalize()
        .unwrap_or_else(|_| lexical_normalize(dest_dir));

    let file = std::fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(true);
    archive.set_unpack_xattrs(false);

    for entry in archive.entries()? {
        let mut entry = entry.map_err(|e| CloakError::Extraction(e.to_string()))?;
        let entry_type = entry.header().entry_type();
        let name = entry
            .path()
            .map_err(|e| CloakError::Extraction(e.to_string()))?
            .into_owned();

        if entry_type.is_symlink() || entry_type.is_hard_link() {
            // Allow symlinks — macOS .app bundles require them. Reject absolute
            // or `..`-containing link targets.
            let link_target = entry
                .link_name()
                .map_err(|e| CloakError::Extraction(e.to_string()))?;
            let suspicious = match &link_target {
                Some(target) => {
                    target.is_absolute()
                        || target
                            .components()
                            .any(|c| matches!(c, std::path::Component::ParentDir))
                }
                None => true,
            };
            if suspicious {
                tracing::warn!(
                    "Skipping suspicious symlink: {} -> {}",
                    name.display(),
                    link_target
                        .as_deref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                );
                continue;
            }
        } else {
            let member_path = dest_dir.join(&name);
            if !is_within(&dest_resolved, &member_path) {
                return Err(CloakError::PathTraversal(name.display().to_string()));
            }
        }

        entry
            .unpack_in(dest_dir)
            .map_err(|e| CloakError::Extraction(e.to_string()))?;
    }
    Ok(())
}

fn extract_zip(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let dest_resolved = dest_dir
        .canonicalize()
        .unwrap_or_else(|_| lexical_normalize(dest_dir));

    let file = std::fs::File::open(archive_path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| CloakError::Extraction(e.to_string()))?;

    // Traversal guard pass first (mirrors Python: check all names, then extract).
    for i in 0..zip.len() {
        let entry = zip
            .by_index(i)
            .map_err(|e| CloakError::Extraction(e.to_string()))?;
        let name = entry.name().to_string();
        let member_path = dest_dir.join(&name);
        if !is_within(&dest_resolved, &member_path) {
            return Err(CloakError::PathTraversal(name));
        }
    }

    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| CloakError::Extraction(e.to_string()))?;
        let Some(enclosed) = entry.enclosed_name() else {
            return Err(CloakError::PathTraversal(entry.name().to_string()));
        };
        let out_path = dest_dir.join(enclosed);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&out_path)?;
        std::io::copy(&mut entry, &mut out)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(mode));
            }
        }
    }
    Ok(())
}

/// If extraction created a single subdirectory, move its contents up one level.
/// Never flatten `.app` bundles — macOS needs the bundle structure intact.
fn flatten_single_subdir(dest_dir: &Path) -> Result<()> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dest_dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();

    if entries.len() != 1 {
        return Ok(());
    }
    let subdir = &entries[0];
    if !subdir.is_dir() {
        return Ok(());
    }
    let name = subdir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.ends_with(".app") {
        tracing::debug!("Keeping .app bundle intact: {}", name);
        return Ok(());
    }

    tracing::debug!("Flattening single subdirectory: {}", name);
    for item in std::fs::read_dir(subdir)? {
        let item = item?.path();
        let file_name = item
            .file_name()
            .ok_or_else(|| CloakError::Extraction("archive entry has no file name".to_string()))?;
        let target = dest_dir.join(file_name);
        std::fs::rename(&item, &target)?;
    }
    std::fs::remove_dir(subdir)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Executable bit / quarantine
// ---------------------------------------------------------------------------

/// `os.access(path, X_OK)` equivalent: on unix, any execute bit set; on
/// Windows, true for any existing file.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(meta) => meta.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        path.exists()
    }
}

/// chmod `mode | 0o111`. No-op on Windows.
fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(perms.mode() | 0o111);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// macOS only: `xattr -cr <path>`. try/except, non-fatal.
#[cfg(target_os = "macos")]
fn remove_quarantine(path: &Path) {
    match std::process::Command::new("xattr")
        .arg("-cr")
        .arg(path)
        .output()
    {
        Ok(_) => tracing::debug!("Removed quarantine attributes from {}", path.display()),
        Err(_) => tracing::debug!("Failed to remove quarantine attributes"),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_checksums_basic() {
        let hash = "a".repeat(64);
        let text = format!("{hash}  cloakbrowser-linux-x64.tar.gz\n");
        let map = parse_checksums(&text);
        assert_eq!(map.get("cloakbrowser-linux-x64.tar.gz"), Some(&hash));
    }

    #[test]
    fn parse_checksums_strips_star_and_lowercases() {
        let upper = "A".repeat(64);
        let lower = "a".repeat(64);
        let text = format!("{upper} *cloakbrowser-windows-x64.zip");
        let map = parse_checksums(&text);
        assert_eq!(map.get("cloakbrowser-windows-x64.zip"), Some(&lower));
    }

    #[test]
    fn parse_checksums_ignores_version_and_junk_lines() {
        let hash = "b".repeat(64);
        let text = format!(
            "version=146.0.7680.177.5\n\
             {hash}  cloakbrowser-linux-x64.tar.gz\n\
             not-a-hash file.txt\n\
             \n\
             deadbeef short.bin\n"
        );
        let map = parse_checksums(&text);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("cloakbrowser-linux-x64.tar.gz"));
        assert!(!map.contains_key("file.txt"));
        assert!(!map.contains_key("short.bin"));
    }

    #[test]
    fn parse_checksums_rejects_non_hex_64() {
        // 64 chars but contains 'g' (non-hex) → rejected.
        let bad = "g".repeat(64);
        let text = format!("{bad}  file.bin");
        let map = parse_checksums(&text);
        assert!(map.is_empty());
    }

    #[test]
    fn parse_checksums_filename_with_spaces() {
        let hash = "c".repeat(64);
        let text = format!("{hash}  weird name.tar.gz");
        let map = parse_checksums(&text);
        assert_eq!(map.get("weird name.tar.gz"), Some(&hash));
    }

    #[test]
    fn parse_manifest_version_reads_line() {
        let text = "version=146.0.7680.177.5\nabc  file";
        assert_eq!(
            parse_manifest_version(text).as_deref(),
            Some("146.0.7680.177.5")
        );
    }

    #[test]
    fn parse_manifest_version_absent() {
        let text = "abc  file\nother line";
        assert_eq!(parse_manifest_version(text), None);
    }

    #[test]
    fn flatten_single_subdir_moves_contents_up() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path();
        let sub = dest.join("wrapper");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("chrome"), b"binary").unwrap();
        std::fs::create_dir(sub.join("locales")).unwrap();

        flatten_single_subdir(dest).unwrap();

        assert!(dest.join("chrome").exists());
        assert!(dest.join("locales").is_dir());
        assert!(!dest.join("wrapper").exists());
    }

    #[test]
    fn flatten_skips_app_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path();
        let app = dest.join("Chromium.app");
        std::fs::create_dir(&app).unwrap();
        std::fs::write(app.join("Info.plist"), b"x").unwrap();

        flatten_single_subdir(dest).unwrap();

        // .app bundle kept intact.
        assert!(dest.join("Chromium.app").is_dir());
        assert!(dest.join("Chromium.app").join("Info.plist").exists());
    }

    #[test]
    fn flatten_noop_multiple_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path();
        std::fs::write(dest.join("a"), b"1").unwrap();
        std::fs::create_dir(dest.join("b")).unwrap();

        flatten_single_subdir(dest).unwrap();

        assert!(dest.join("a").exists());
        assert!(dest.join("b").is_dir());
    }

    #[test]
    fn flatten_noop_single_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path();
        std::fs::write(dest.join("chrome"), b"1").unwrap();

        flatten_single_subdir(dest).unwrap();

        assert!(dest.join("chrome").exists());
    }

    #[test]
    fn is_within_rejects_traversal() {
        let dest = PathBuf::from("/cache/chromium-1");
        assert!(is_within(&dest, &PathBuf::from("/cache/chromium-1/chrome")));
        assert!(!is_within(&dest, &PathBuf::from("/cache/chromium-1/../evil")));
    }
}
