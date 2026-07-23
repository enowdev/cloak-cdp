# CloakBrowser DOWNLOAD + VERIFY + EXTRACT — Rust Port Spec (free public path only)

Source: `cloakbrowser/download.py` (1188 lines) + `cloakbrowser/config.py`. Pro-binary auth paths excluded. Free public download path fully documented.

## 0. Constants (from config.py)

```
CHROMIUM_VERSION      = "146.0.7680.177.5"     # latest across all platforms (display/fallback)
PLATFORM_CHROMIUM_VERSIONS = {
    "linux-x64":    "146.0.7680.177.5",
    "linux-arm64":  "146.0.7680.177.3",
    "darwin-arm64": "145.0.7632.109.2",
    "darwin-x64":   "145.0.7632.109.2",
    "windows-x64":  "146.0.7680.177.5",
}
AVAILABLE_PLATFORMS = keys of the above map

BINARY_SIGNING_PUBKEYS = [ "MKFKwIhUcKWq5xTuNA0Ovg99njcDEcEJvmWYYhApvaU=" ]   # base64 of 32-byte Ed25519 raw public key

DOWNLOAD_BASE_URL         = env CLOAKBROWSER_DOWNLOAD_URL  OR  "https://cloakbrowser.dev"
GITHUB_API_URL            = "https://api.github.com/repos/CloakHQ/cloakbrowser/releases"
GITHUB_DOWNLOAD_BASE_URL  = "https://github.com/CloakHQ/cloakbrowser/releases/download"

DOWNLOAD_TIMEOUT  = connect=10s, read=60s, write=10s, pool=10s
UPDATE_CHECK_INTERVAL = 3600  (periodic-check marker — note only)

_VERSION_PIN_RE = ^[0-9]+(?:\.[0-9]+){3,4}$    # 4 or 5 dotted numeric groups
```

### Platform detection (`get_platform_tag`)
Python uses `platform.system()` = "Linux"/"Darwin"/"Windows", `platform.machine()`.
```
("Linux",   "x86_64"): "linux-x64"
("Linux",   "aarch64"): "linux-arm64"
("Darwin",  "arm64"):   "darwin-arm64"
("Darwin",  "x86_64"):  "darwin-x64"
("Windows", "AMD64"):   "windows-x64"
("Windows", "x86_64"):  "windows-x64"
```
Unknown pair → hard error. In Rust map target_os/target_arch:
linux+x86_64→linux-x64, linux+aarch64→linux-arm64, macos+aarch64→darwin-arm64, macos+x86_64→darwin-x64, windows+x86_64→windows-x64.

### Archive naming
```
get_archive_ext()  = ".zip" if Windows else ".tar.gz"
get_archive_name(tag=current) = f"cloakbrowser-{tag}{ext}"
    e.g.  "cloakbrowser-linux-x64.tar.gz", "cloakbrowser-windows-x64.zip", "cloakbrowser-darwin-arm64.tar.gz"
```

### Version resolution helpers
- `get_chromium_version()` = `PLATFORM_CHROMIUM_VERSIONS.get(tag, CHROMIUM_VERSION)`.
- `normalize_requested_version(arg)`: value = arg else env CLOAKBROWSER_VERSION. None/empty → None. Strip whitespace. Must match `_VERSION_PIN_RE` else ValueError.
- `_version_tuple(v)` = split on `.` → tuple of ints. `_version_newer(a,b)` = `tuple(a) > tuple(b)`.
- `get_effective_version(pro=False)` free: base = get_chromium_version(). Read markers `latest_version_{tag}` then legacy `latest_version` from cache dir; if non-empty AND newer AND get_binary_path(version).exists() → return version. Else base.

### Paths
```
get_cache_dir()  = env CLOAKBROWSER_CACHE_DIR  OR  ~/.cloakbrowser
get_binary_dir(version=current, pro=False) = cache_dir / f"chromium-{version}"   # free: NO "-pro" suffix
get_binary_path(version, pro=False):
    Darwin:  binary_dir / "Chromium.app" / "Contents" / "MacOS" / "Chromium"
    Windows: binary_dir / "chrome.exe"
    Linux:   binary_dir / "chrome"
```
NOTE: cache subdir is `chromium-{version}` (no `v`), but download URLs use `chromium-v{version}` (with `v`).

### Env vars honored
| Env var | Effect |
|---|---|
| CLOAKBROWSER_BINARY_PATH | Local override; skip all download/verify/extract, return this path (must exist). |
| CLOAKBROWSER_CACHE_DIR | Cache root (default ~/.cloakbrowser). |
| CLOAKBROWSER_DOWNLOAD_URL | Overrides DOWNLOAD_BASE_URL. When set: (a) disables GitHub fallback, (b) switches verification to legacy same-origin checksum path (no signature), (c) disables periodic update checks. Forces free path. |
| CLOAKBROWSER_SKIP_CHECKSUM | Only when CLOAKBROWSER_DOWNLOAD_URL set; "true" (case-insensitive) skips checksum. Does NOT bypass official Ed25519 signature. |
| CLOAKBROWSER_VERSION | Version pin. |
| CLOAKBROWSER_AUTO_UPDATE | "false" disables periodic/background update checks. |

## 1. ensure_binary(license_key=None, browser_version=None) -> str  (free path)

1. Local override: if CLOAKBROWSER_BINARY_PATH set → error if missing, else return it.
2. requested_version = normalize_requested_version(browser_version).
3. (Pro branch excluded; if CLOAKBROWSER_DOWNLOAD_URL set, key forced None → free path.)
4. check_platform_available(): tag not in AVAILABLE_PLATFORMS → error/exit.
5. Pinned path (requested set): binary_path=get_binary_path(requested). If exists+executable → return. Else _download_and_extract(requested). Re-check; if missing → RuntimeError. Return str.
6. Unpinned: effective=get_effective_version(); binary_path=get_binary_path(effective). If exists+executable → return. Else if effective != platform_version try fallback get_binary_path() hardcoded. Else _download_and_extract(). Re-check; return.

_is_executable(path) = os.access(path, X_OK). Rust: mode & 0o111 != 0 on unix; on Windows always true for existing files.

## 2. Download

```
primary_url  = f"{DOWNLOAD_BASE_URL}/chromium-v{v}/{archive_name}"
fallback_url = f"{GITHUB_DOWNLOAD_BASE_URL}/chromium-v{v}/{archive_name}"
```
Example (linux-x64): primary `https://cloakbrowser.dev/chromium-v146.0.7680.177.5/cloakbrowser-linux-x64.tar.gz`, fallback `https://github.com/CloakHQ/cloakbrowser/releases/download/chromium-v146.0.7680.177.5/cloakbrowser-linux-x64.tar.gz`.

_download_and_extract(version=None):
1. Compute urls, binary_dir, binary_path.
2. binary_dir.parent.mkdir(parents=True, exist_ok=True).
3. NamedTemporaryFile suffix=ext, delete=False → tmp_path (system temp dir).
4. try _download_file(primary_url, tmp). except: if CLOAKBROWSER_DOWNLOAD_URL set → re-raise (no fallback); else _download_file(fallback_url, tmp). NO per-request retry loop.
5. _verify_download_checksum(tmp, version).
6. _extract_archive(tmp, binary_dir, binary_path).
7. finally tmp.unlink(missing_ok=True).

_download_file(url, dest): streaming GET, follow_redirects, timeout=DOWNLOAD_TIMEOUT, raise_for_status, 8192-byte chunks → dest "wb".

## 3. Verification (two mutually exclusive paths)

```
tarball_name = get_archive_name()

if env CLOAKBROWSER_DOWNLOAD_URL set:
    # self-hosted: checksum only
    if env CLOAKBROWSER_SKIP_CHECKSUM.lower()=="true": return
    checksums = _fetch_checksums(version); if None: return
    expected = checksums.get(tarball_name); if None: return
    _verify_checksum(file, expected); return
else:
    # official: mandatory Ed25519
    manifest = _fetch_signed_manifest(version)  # (SHA256SUMS bytes, SHA256SUMS.sig bytes)
    if None: raise RuntimeError("Could not fetch a signed SHA256SUMS ... refusing")
    _verify_signature(manifest_bytes, sig_bytes)   # Ed25519
    declared = _parse_manifest_version(manifest_text); requested = version or get_chromium_version()
    if declared != requested: raise RuntimeError("Version mismatch ... downgrade")
    checksums = _parse_checksums(manifest_text); expected = checksums.get(tarball_name)
    if None: raise RuntimeError("Signature-verified SHA256SUMS has no entry")
    _verify_checksum(file, expected)
```

**Signature scheme: raw Ed25519** (NOT minisign/gpg). Detached sig = base64 of 64-byte raw signature. Pubkey embedded as BINARY_SIGNING_PUBKEYS (base64 of 32-byte raw Ed25519 pubkey). Message signed = raw SHA256SUMS file bytes, unmodified.

_fetch_signed_manifest(version): for base in [DOWNLOAD_BASE_URL/chromium-v{v}, GITHUB_DOWNLOAD_BASE_URL/chromium-v{v}]: GET {base}/SHA256SUMS and {base}/SHA256SUMS.sig (follow_redirects, timeout=10s, raise_for_status); return (manifest.content, sig.content). Both from SAME origin per iteration. Returns None if all fail.

_verify_signature Rust: base64 decode sig (strict) → 64 bytes → ed25519_dalek Signature. Each pinned key: base64 decode → 32 bytes → VerifyingKey → verify_strict(manifest_bytes, &sig). Succeed on first match; skip unparsable keys; fail closed.

_parse_manifest_version(text): first line starting "version=" → value after "version=". None if absent. declared must == requested (exact string equality) else RuntimeError.

_parse_checksums(text):
```python
result = {}
for line in text.strip().splitlines():
    parts = line.strip().split(None, 1)
    if len(parts) != 2: continue
    hash_val, filename = parts
    hash_val = hash_val.lower()
    if len(hash_val) != 64 or any(c not in "0123456789abcdef" for c in hash_val): continue
    result[filename.lstrip("*")] = hash_val
return result
```
Format: `<64-hex-sha256><whitespace><filename>`. Lookup key = exact tarball_name.

_verify_checksum(file, expected): SHA-256 streamed 8192-byte chunks, hex lowercase == expected else RuntimeError.

## 4. Extraction

_extract_archive(archive, dest_dir, binary_path=None):
1. if dest_dir.exists() → rmtree.
2. mkdir parents.
3. .zip → _extract_zip; else _extract_tar.
4. _flatten_single_subdir(dest_dir).
5. bp = binary_path or get_binary_path(); if exists → _make_executable(bp).
6. if Darwin → _remove_quarantine(dest_dir).

_extract_tar (tar.gz, "r:gz"): for each member:
- symlink/hardlink: link_target=member.linkname. If os.path.isabs(link_target) OR ".." in link_target.split("/") → skip (warn). Else keep. (Symlinks allowed for macOS .app Framework layout.)
- else regular: member_path=(dest_dir/member.name).resolve(). If not str(member_path).startswith(str(dest_dir.resolve())) → RuntimeError "path traversal". Else keep.
- tar.extractall(dest_dir, members=safe_members).
Rust: flate2::GzDecoder + tar::Archive; preserve symlinks, reject absolute/`..` link targets, confirm regular entries stay within dest.

_extract_zip: for each info in infolist(): member_path=(dest_dir/info.filename).resolve(); if not under dest → RuntimeError. Then extractall. Rust: zip crate.

_flatten_single_subdir(dest_dir):
```python
entries = list(dest_dir.iterdir())
if len(entries) == 1 and entries[0].is_dir():
    subdir = entries[0]
    if subdir.name.endswith(".app"): return   # never flatten .app
    for item in subdir.iterdir(): shutil.move(item, dest_dir / item.name)
    subdir.rmdir()
```

_make_executable(path): Windows → no-op. Else chmod(mode | 0o111). Rust: PermissionsExt.

_remove_quarantine(path) macOS only: subprocess.run(["xattr","-cr",str(path)], capture_output=True, timeout=30). try/except, non-fatal.

## 5. Update checking (note only — do NOT port the periodic marker machinery for first port)
_maybe_trigger_update_check spawns daemon background threads; never blocks; never affects returned path. Optional. Uses GITHUB_API_URL ?per_page=10 to find newest chromium-v* release; writes latest_version_{tag} markers. Skip for first port.

## Crates
reqwest (HTTP, redirects, timeouts, streaming), ed25519-dalek (signature), base64, sha2, flate2 + tar (tar.gz), zip (Windows), dirs/home (~/.cloakbrowser), tempfile.

Load-bearing strings: cache `~/.cloakbrowser`; binary subdir `chromium-{version}` (no v); URL segment `chromium-v{version}` (with v); archive `cloakbrowser-{tag}{.tar.gz|.zip}`; manifest `SHA256SUMS` + `SHA256SUMS.sig`; pinned pubkey `MKFKwIhUcKWq5xTuNA0Ovg99njcDEcEJvmWYYhApvaU=`; manifest version line `version=<v>`; chunk 8192; timeouts 10/60/10/10s; manifest fetch 10s.
