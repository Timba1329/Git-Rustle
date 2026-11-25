//! Self-update from GitHub releases.
//!
//! Flow: ask the GitHub API for the latest release tag, and if it is newer, download
//! `Rustle-<version>-portable.zip` and its `.sha256` file from that release, verify the checksum,
//! take `rustle.exe` out of the zip and swap it in for the running executable (the running file is
//! renamed to `*.old`, which Windows allows). The caller restarts the app afterwards.
//!
//! The checksum protects against corrupt or truncated downloads. It does not protect against a
//! compromised release (that would need code signing).

use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

const EXE_NAME: &str = "rustle.exe";
/// Upper bound for what we are willing to download or unpack.
const MAX_BYTES: u64 = 150 * 1024 * 1024;

/// Messages from the background threads to the UI.
pub enum Msg {
    /// Result of asking GitHub for the latest release tag.
    Checked(Result<String, String>),
    /// Download progress between 0.0 and 1.0.
    Progress(f32),
    /// The new executable is in place.
    Installed,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub zip_url: String,
    pub sha_url: String,
    pub page_url: String,
}

impl Release {
    /// Builds the download addresses from the tag; the release workflow publishes assets under
    /// these names.
    pub fn from_tag(repo: &str, tag: &str) -> Self {
        let version = tag.trim_start_matches(['v', 'V']);
        let zip = format!("https://github.com/{repo}/releases/download/{tag}/Rustle-{version}-portable.zip");
        Self {
            tag: tag.to_string(),
            sha_url: format!("{zip}.sha256"),
            zip_url: zip,
            page_url: format!("https://github.com/{repo}/releases/tag/{tag}"),
        }
    }
}

/// Only downloads from this release path are accepted.
pub fn allowed_prefix(repo: &str) -> String {
    format!("https://github.com/{repo}/releases/download/")
}

/// `https://github.com/owner/repo(.git)`, `git@github.com:owner/repo.git` or `owner/repo` → `owner/repo`.
pub fn repo_from_url(url: &str) -> String {
    let mut s = url.trim();
    for p in ["https://github.com/", "http://github.com/", "git@github.com:"] {
        if let Some(rest) = s.strip_prefix(p) {
            s = rest;
            break;
        }
    }
    s.trim_end_matches('/')
        .trim_end_matches(".git")
        .trim_matches('/')
        .to_string()
}

/// The repository this build was made from (`repository` in Cargo.toml).
pub fn default_repo() -> String {
    repo_from_url(env!("CARGO_PKG_REPOSITORY"))
}

/// The repository to check: the user's override, or the built-in default.
pub fn effective_repo(user_override: &str) -> String {
    let o = user_override.trim();
    if o.is_empty() {
        default_repo()
    } else {
        repo_from_url(o)
    }
}

/// Extracts the value of `"tag_name"` from a GitHub release JSON document.
pub fn parse_tag(json: &str) -> Option<String> {
    let rest = &json[json.find("\"tag_name\"")? + "\"tag_name\"".len()..];
    let rest = &rest[rest.find('"')? + 1..];
    Some(rest[..rest.find('"')?].to_string())
}

fn parts(v: &str) -> Vec<u64> {
    v.trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// Whether `latest` is a higher version than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (a, b) = (parts(latest), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

fn agent(timeout_secs: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .build()
        .into()
}

fn get(url: &str, timeout_secs: u64) -> Result<ureq::http::Response<ureq::Body>, String> {
    agent(timeout_secs)
        .get(url)
        .header("User-Agent", "Rustle-updater")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())
}

/// Asks GitHub for the tag of the latest release.
pub fn fetch_latest_tag(repo: &str) -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo.trim());
    let body = get(&url, 10)?
        .into_body()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse_tag(&body).ok_or_else(|| "no release found".to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of `data` as lowercase hex.
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// First token of a `.sha256` file (`<hash>  <filename>`), if it looks like a SHA-256 digest.
pub fn parse_sha(text: &str) -> Option<String> {
    let t = text.split_whitespace().next()?.to_lowercase();
    (t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())).then_some(t)
}

fn download(url: &str, progress: &mut dyn FnMut(f32)) -> Result<Vec<u8>, String> {
    let resp = get(url, 900).map_err(|e| format!("download failed: {e}"))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if total.is_some_and(|t| t > MAX_BYTES) {
        return Err("the update is unexpectedly large".into());
    }
    let mut reader = resp.into_body().into_reader();
    let mut data = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if data.len() as u64 > MAX_BYTES {
            return Err("the update is unexpectedly large".into());
        }
        if let Some(t) = total.filter(|t| *t > 0) {
            progress((data.len() as f32 / t as f32).min(1.0));
        }
    }
    Ok(data)
}

/// Takes `rustle.exe` out of the release zip.
pub fn extract_exe(zip_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(zip_bytes)).map_err(|e| format!("invalid update archive: {e}"))?;
    let mut file = archive
        .by_name(EXE_NAME)
        .map_err(|_| format!("the update archive does not contain {EXE_NAME}"))?;
    if file.size() > MAX_BYTES {
        return Err("the update is unexpectedly large".into());
    }
    let mut out = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut out)
        .map_err(|e| format!("could not unpack the update: {e}"))?;
    Ok(out)
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// Replaces `target` with `bytes`. The old file is moved to `<target>.old` (a running executable
/// can be renamed on Windows, but not overwritten). Nothing is touched when the new file cannot be
/// written, and the old file is restored if the final rename fails.
pub fn replace_file(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let new = with_suffix(target, ".new");
    let old = with_suffix(target, ".old");
    std::fs::write(&new, bytes).map_err(|e| format!("cannot write next to the application ({e})"))?;
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(target, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("cannot replace the application ({e})"));
    }
    if let Err(e) = std::fs::rename(&new, target) {
        let _ = std::fs::rename(&old, target);
        let _ = std::fs::remove_file(&new);
        return Err(format!("cannot put the new version in place ({e})"));
    }
    Ok(())
}

/// Removes leftovers of a previous update. Call at startup.
pub fn cleanup_old(exe: &Path) {
    let _ = std::fs::remove_file(with_suffix(exe, ".old"));
    let _ = std::fs::remove_file(with_suffix(exe, ".new"));
}

/// Downloads, verifies and installs a release over `exe`.
///
/// `allowed_prefix` restricts the addresses that may be downloaded from; `progress` receives 0..1.
pub fn install(
    rel: &Release,
    exe: &Path,
    allowed_prefix: &str,
    progress: &mut dyn FnMut(f32),
) -> Result<(), String> {
    for u in [&rel.zip_url, &rel.sha_url] {
        if !u.starts_with(allowed_prefix) {
            return Err("refusing to download from an unexpected address".into());
        }
    }
    let sha_text = get(&rel.sha_url, 30)
        .map_err(|e| format!("could not download the checksum: {e}"))?
        .into_body()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let expected = parse_sha(&sha_text).ok_or("the checksum file is not valid")?;

    let zip_bytes = download(&rel.zip_url, progress)?;
    let actual = sha256_hex(&zip_bytes);
    if actual != expected {
        return Err("the downloaded update is corrupt (checksum mismatch); nothing was changed".into());
    }
    let exe_bytes = extract_exe(&zip_bytes)?;
    if cfg!(windows) && !exe_bytes.starts_with(b"MZ") {
        return Err("the update does not contain a Windows executable".into());
    }
    replace_file(exe, &exe_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn parses_tag() {
        let j = r#"{"url":"x","tag_name": "v1.4.2","name":"Release"}"#;
        assert_eq!(parse_tag(j).as_deref(), Some("v1.4.2"));
        assert_eq!(parse_tag("{}"), None);
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("v1.2.0", "1.1.9"));
        assert!(is_newer("0.10.0", "0.9.5"));
        assert!(!is_newer("v1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
        assert!(is_newer("1.0.1", "1.0"));
    }

    #[test]
    fn repo_names() {
        assert_eq!(
            repo_from_url("https://github.com/Timba1329/Git-Rustle.git"),
            "Timba1329/Git-Rustle"
        );
        assert_eq!(
            repo_from_url("https://github.com/Timba1329/Git-Rustle/"),
            "Timba1329/Git-Rustle"
        );
        assert_eq!(repo_from_url("git@github.com:a/b.git"), "a/b");
        assert_eq!(repo_from_url("a/b"), "a/b");
        assert_eq!(default_repo(), "Timba1329/Git-Rustle");
        assert_eq!(effective_repo(""), "Timba1329/Git-Rustle");
        assert_eq!(effective_repo("  x/y "), "x/y");
    }

    #[test]
    fn release_urls_follow_the_asset_names() {
        let r = Release::from_tag("o/r", "v1.2.3");
        assert_eq!(
            r.zip_url,
            "https://github.com/o/r/releases/download/v1.2.3/Rustle-1.2.3-portable.zip"
        );
        assert_eq!(r.sha_url, format!("{}.sha256", r.zip_url));
        assert!(r.zip_url.starts_with(&allowed_prefix("o/r")));
        assert_eq!(r.page_url, "https://github.com/o/r/releases/tag/v1.2.3");
    }

    #[test]
    fn checksum_helpers() {
        // SHA-256 of "abc".
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let h =
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD  Rustle-1.0.0-portable.zip\n";
        assert_eq!(
            parse_sha(h).as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert_eq!(parse_sha("not a hash"), None);
        assert_eq!(parse_sha(""), None);
    }

    fn make_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_the_exe() {
        let z = make_zip(&[("LICENSE", b"mit"), ("rustle.exe", b"MZ-new-binary")]);
        assert_eq!(extract_exe(&z).unwrap(), b"MZ-new-binary");
        let no_exe = make_zip(&[("README.md", b"x")]);
        assert!(extract_exe(&no_exe).unwrap_err().contains("does not contain"));
        assert!(extract_exe(b"not a zip").is_err());
    }

    #[test]
    fn replace_keeps_old_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("rustle.exe");
        std::fs::write(&exe, b"old").unwrap();
        replace_file(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(with_suffix(&exe, ".old")).unwrap(), b"old");
        assert!(!with_suffix(&exe, ".new").exists());
        cleanup_old(&exe);
        assert!(!with_suffix(&exe, ".old").exists());
    }

    #[test]
    fn replace_fails_cleanly_when_the_folder_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("gone").join("rustle.exe");
        assert!(replace_file(&exe, b"new").is_err());
    }

    /// Serves `routes` (path → body) on a random local port for `requests` requests.
    fn serve(routes: Vec<(String, Vec<u8>)>, requests: usize) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..requests {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                let body = routes.iter().find(|(p, _)| *p == path).map(|(_, b)| b.clone());
                let head = match &body {
                    Some(b) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        b.len()
                    ),
                    None => {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                    }
                };
                let _ = s.write_all(head.as_bytes());
                if let Some(b) = body {
                    let _ = s.write_all(&b);
                }
            }
        });
        port
    }

    fn local_release(port: u16) -> (Release, String) {
        let base = format!("http://127.0.0.1:{port}/o/r/releases/download/v9.0.0");
        let zip_url = format!("{base}/Rustle-9.0.0-portable.zip");
        (
            Release {
                tag: "v9.0.0".into(),
                sha_url: format!("{zip_url}.sha256"),
                zip_url,
                page_url: String::new(),
            },
            format!("http://127.0.0.1:{port}/o/r/releases/download/"),
        )
    }

    #[test]
    fn installs_a_release_end_to_end() {
        let zip = make_zip(&[("rustle.exe", b"MZ-new-binary"), ("LICENSE", b"mit")]);
        let sum = format!("{}  Rustle-9.0.0-portable.zip\n", sha256_hex(&zip));
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("rustle.exe");
        std::fs::write(&exe, b"MZ-old-binary").unwrap();

        let base = "/o/r/releases/download/v9.0.0/Rustle-9.0.0-portable.zip".to_string();
        let port = serve(
            vec![(base.clone(), zip), (format!("{base}.sha256"), sum.into_bytes())],
            2,
        );
        let (rel, prefix) = local_release(port);
        let mut last = 0.0;
        install(&rel, &exe, &prefix, &mut |p| last = p).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"MZ-new-binary");
        assert!((last - 1.0).abs() < f32::EPSILON, "progress reached {last}");
    }

    #[test]
    fn rejects_a_checksum_mismatch_without_touching_the_exe() {
        let zip = make_zip(&[("rustle.exe", b"MZ-evil")]);
        let wrong = format!("{}  x.zip\n", sha256_hex(b"something else"));
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("rustle.exe");
        std::fs::write(&exe, b"MZ-old-binary").unwrap();

        let base = "/o/r/releases/download/v9.0.0/Rustle-9.0.0-portable.zip".to_string();
        let port = serve(
            vec![
                (base.clone(), zip),
                (format!("{base}.sha256"), wrong.into_bytes()),
            ],
            2,
        );
        let (rel, prefix) = local_release(port);
        let err = install(&rel, &exe, &prefix, &mut |_| {}).unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"MZ-old-binary");
        assert!(!with_suffix(&exe, ".old").exists());
    }

    #[test]
    fn refuses_unexpected_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("rustle.exe");
        std::fs::write(&exe, b"old").unwrap();
        let rel = Release::from_tag("evil/repo", "v9.9.9");
        let err = install(&rel, &exe, &allowed_prefix("Timba1329/Git-Rustle"), &mut |_| {}).unwrap_err();
        assert!(err.contains("unexpected address"));
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    }

    #[test]
    fn a_missing_asset_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("rustle.exe");
        std::fs::write(&exe, b"old").unwrap();
        let port = serve(vec![], 1);
        let (rel, prefix) = local_release(port);
        assert!(install(&rel, &exe, &prefix, &mut |_| {}).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    }
}
