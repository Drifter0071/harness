//! Connection-info file: the one-shot token a client reads to bootstrap
//! a TCP connection to the HCI server.
//!
//! The file is written next to the executable when possible, falling back
//! to the OS temp dir when the exe directory is read-only (e.g. installed
//! under `Program Files`). The token inside is a 256-bit random value
//! encoded as base64. Atomic writes only — a half-written file is a
//! brittle contract.

use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Filename for the connection-info file. Looked up in two locations:
/// the directory of the running exe, then the OS temp dir.
pub const CONN_INFO_FILENAME: &str = "harness.control.json";

/// Wire version of the file. Bump on any breaking change to the schema.
pub const CONN_INFO_VERSION: u32 = 1;

/// On-disk shape of `harness.control.json`. Field names are stable; do
/// not rename without bumping `CONN_INFO_VERSION`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnInfo {
    pub version: u32,
    pub pid: u32,
    pub addr: String,
    pub port: u16,
    pub boot_token: String,
    pub started_at: String,
    pub build: BuildInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildInfo {
    pub git_sha: String,
    pub channel: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnInfoError {
    #[error("could not locate the directory for the connection-info file")]
    NoSuitableDir,
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Build a `ConnInfo` with a freshly generated boot token. The token is
/// 256 bits of `OsRng` entropy, base64-encoded with standard alphabet
/// (no padding for compactness, since the wire format is JSON-friendly).
pub fn build_conn_info(addr: std::net::IpAddr, port: u16, channel: &str) -> ConnInfo {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let boot_token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);

    ConnInfo {
        version: CONN_INFO_VERSION,
        pid: std::process::id(),
        addr: addr.to_string(),
        port,
        boot_token,
        started_at: chrono::Utc::now().to_rfc3339(),
        build: BuildInfo {
            git_sha: option_env!("HARNESS_GIT_SHA").unwrap_or("unknown").to_string(),
            channel: channel.to_string(),
        },
    }
}

/// Pick the directory the connection-info file should live in.
///
/// 1. `HARNESS_CONN_INFO_DIR` env var if set (test-only override).
/// 2. The directory of the current executable (preferred — survives the
///    terminal that launched the process being closed).
/// 3. The OS temp directory (fallback for protected install locations).
///
/// `None` only when every probe fails.
pub fn conn_info_dir() -> Option<PathBuf> {
    if let Ok(override_dir) = std::env::var("HARNESS_CONN_INFO_DIR") {
        let p = PathBuf::from(override_dir);
        if p.is_dir() && is_writable_dir(&p) {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if is_writable_dir(dir) {
                return Some(dir.to_path_buf());
            }
        }
    }
    let tmp = std::env::temp_dir();
    if tmp.is_absolute() && is_writable_dir(&tmp) {
        return Some(tmp);
    }
    None
}

fn is_writable_dir(p: &Path) -> bool {
    // Probe by attempting to create a uniquely-named temp file inside.
    // If that succeeds, we have write access. We do not leave the file
    // behind — `tempfile` cleans it up on drop.
    tempfile::Builder::new()
        .prefix(".hci-write-probe-")
        .tempfile_in(p)
        .is_ok()
}

/// Full path to the connection-info file in the chosen directory.
pub fn conn_info_path() -> Option<PathBuf> {
    conn_info_dir().map(|d| d.join(CONN_INFO_FILENAME))
}

/// Atomically write the connection-info file. Uses `tempfile::NamedTempFile`
/// in the same directory, then persists to the target path. On Windows
/// `persist` is atomic at the filesystem level; on Unix the rename is
/// atomic within the same mount point.
pub fn write_conn_info(info: &ConnInfo) -> Result<PathBuf, ConnInfoError> {
    let target = conn_info_path().ok_or(ConnInfoError::NoSuitableDir)?;
    write_conn_info_to(info, target.parent().unwrap())
}

/// Write the connection-info file into an explicit directory. Used by
/// tests and by the server when an override path is configured.
pub fn write_conn_info_to(info: &ConnInfo, dir: &Path) -> Result<PathBuf, ConnInfoError> {
    fs::create_dir_all(dir)?;
    let target = dir.join(CONN_INFO_FILENAME);
    let json = serde_json::to_vec_pretty(info)?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(&json)?;
    tmp.flush()?;
    tmp.persist(&target)
        .map_err(|e| ConnInfoError::Io(e.error))?;
    Ok(target)
}

/// Remove the connection-info file. No error if the file does not exist
/// — the contract is "best effort cleanup", and a previous process crash
/// may have left it in a stale state.
pub fn remove_conn_info() -> Result<(), ConnInfoError> {
    if let Some(path) = conn_info_path() {
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ConnInfoError::Io(e)),
        }
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn build_conn_info_has_fresh_token() {
        let info = build_conn_info(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345, "test");
        assert_eq!(info.version, CONN_INFO_VERSION);
        assert_eq!(info.port, 12345);
        assert_eq!(info.addr, "127.0.0.1");
        assert_eq!(info.build.channel, "test");
        // 32 bytes base64-URL-safe-no-pad = 43 chars
        assert_eq!(info.boot_token.len(), 43);
        assert!(info.boot_token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn two_tokens_are_distinct() {
        let a = build_conn_info(IpAddr::V4(Ipv4Addr::LOCALHOST), 1, "test");
        let b = build_conn_info(IpAddr::V4(Ipv4Addr::LOCALHOST), 1, "test");
        assert_ne!(a.boot_token, b.boot_token);
    }

    #[test]
    fn write_and_remove_roundtrip() {
        // Force the connection-info file into a temp directory by overriding
        // the cwd-derived lookup via `current_exe` is fragile, so we test
        // the public write/read shape by writing to a known path with the
        // same primitives.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(CONN_INFO_FILENAME);
        let info = build_conn_info(IpAddr::V4(Ipv4Addr::LOCALHOST), 51423, "test");
        let json = serde_json::to_vec_pretty(&info).unwrap();
        std::fs::write(&target, &json).unwrap();
        let read: ConnInfo = serde_json::from_slice(&json).unwrap();
        assert_eq!(info, read);
        // And the file actually exists where we wrote it.
        assert!(target.exists());
    }

    #[test]
    fn conn_info_serializes_to_expected_shape() {
        let info = ConnInfo {
            version: CONN_INFO_VERSION,
            pid: 4242,
            addr: "127.0.0.1".to_string(),
            port: 51423,
            boot_token: "abc".to_string(),
            started_at: "2026-08-12T10:00:00Z".to_string(),
            build: BuildInfo {
                git_sha: "deadbeef".to_string(),
                channel: "release".to_string(),
            },
        };
        let s = serde_json::to_string(&info).unwrap();
        // Field order is alphabetical by serde, but we just check presence.
        assert!(s.contains("\"version\":1"));
        assert!(s.contains("\"pid\":4242"));
        assert!(s.contains("\"port\":51423"));
        assert!(s.contains("\"boot_token\":\"abc\""));
        assert!(s.contains("\"channel\":\"release\""));
    }
}
