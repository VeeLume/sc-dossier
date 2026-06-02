//! Read credentials from the RSI launcher's `launcher store.json`
//! (electron-store: AES-256-CBC + PBKDF2-HMAC-SHA512, key read at runtime from
//! the launcher's `app.asar`).
//!
//! Surfaced fields:
//! - `session.value` → `X-Rsi-Token` (platform session; may be expired)
//! - `device.value`  → `X-Rsi-Device` (device token)
//! - `library.defaults[SC]` → default channel + platform id
//! - `application.version`  → launcher version

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use aes::Aes256;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use pbkdf2::pbkdf2_hmac;
use regex::bytes::Regex;
use serde_json::Value;
use sha2::Sha512;

use crate::error::{Error, Result};

/// Credentials and context pulled from the launcher store.
#[derive(Clone)]
pub struct Credentials {
    /// `X-Rsi-Token` — the platform session token. May be expired.
    pub session_token: String,
    /// `X-Rsi-Device` — the persistent device token.
    pub device_token: String,
    /// Default SC channel id (e.g. `"LIVE"`), if present.
    pub default_channel: Option<String>,
    /// Platform id for the default channel (`"prod"` / `"ptu"`), if present.
    pub platform_id: Option<String>,
    /// Launcher version (e.g. `"2.13.3"`).
    pub launcher_version: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("session_token", &"<redacted>")
            .field("device_token", &"<redacted>")
            .field("default_channel", &self.default_channel)
            .field("platform_id", &self.platform_id)
            .field("launcher_version", &self.launcher_version)
            .finish()
    }
}

/// Read credentials from the default launcher store / asar locations (Windows).
pub fn read_credentials() -> Result<Credentials> {
    let asar = locate_app_asar()?;
    read_credentials_from(&launcher_store_path(), &asar)
}

/// Like [`read_credentials`] but with explicit paths (tests / non-default installs).
pub fn read_credentials_from(store_path: &Path, asar_path: &Path) -> Result<Credentials> {
    let store_bytes = match std::fs::read(store_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::StoreNotFound(store_path.to_path_buf()))
        }
        Err(e) => return Err(Error::Store(format!("reading {}: {e}", store_path.display()))),
    };
    let key_b64 = extract_encryption_key(asar_path)?;
    let plaintext = decrypt_store(&store_bytes, &key_b64)?;
    let value: Value = serde_json::from_slice(&plaintext)
        .map_err(|e| Error::Store(format!("parsing store JSON: {e}")))?;
    parse_credentials(&value)
}

fn parse_credentials(v: &Value) -> Result<Credentials> {
    let session_token = v
        .pointer("/session/value")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or(Error::MissingCredential { what: "session token" })?;
    let device_token = v
        .pointer("/device/value")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or(Error::MissingCredential { what: "device token" })?;

    let default = v
        .pointer("/library/defaults")
        .and_then(Value::as_array)
        .and_then(|arr| arr.iter().find(|d| d.get("gameId").and_then(Value::as_str) == Some("SC")));

    Ok(Credentials {
        session_token,
        device_token,
        default_channel: default
            .and_then(|d| d.get("channelId"))
            .and_then(Value::as_str)
            .map(str::to_string),
        platform_id: default
            .and_then(|d| d.get("platformId"))
            .and_then(Value::as_str)
            .map(str::to_string),
        launcher_version: v
            .pointer("/application/version")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

// ── Paths ───────────────────────────────────────────────────────────────────

fn launcher_store_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    PathBuf::from(appdata).join("rsilauncher/launcher store.json")
}

fn locate_app_asar() -> Result<PathBuf> {
    let candidates = [
        std::env::var("PROGRAMFILES").ok().map(PathBuf::from),
        std::env::var("ProgramW6432").ok().map(PathBuf::from),
        Some(PathBuf::from(r"C:\Program Files")),
    ];
    for base in candidates.into_iter().flatten() {
        let p = base
            .join("Roberts Space Industries")
            .join("RSI Launcher")
            .join("resources")
            .join("app.asar");
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(Error::LauncherNotFound)
}

// ── Decryption (electron-store) ─────────────────────────────────────────────

fn extract_encryption_key(asar_path: &Path) -> Result<Vec<u8>> {
    let re = Regex::new(r#"encryptionKey:\s*["']([A-Za-z0-9+/]{43}=)["']"#).unwrap();
    let file = File::open(asar_path)
        .map_err(|e| Error::Store(format!("opening {}: {e}", asar_path.display())))?;
    let mut reader = BufReader::with_capacity(1 << 20, file);

    const CHUNK: usize = 1 << 20;
    const OVERLAP: usize = 128;
    let mut buf = vec![0u8; CHUNK + OVERLAP];
    let mut carry = 0usize;

    loop {
        let n = reader
            .read(&mut buf[carry..])
            .map_err(|e| Error::Store(format!("reading asar: {e}")))?;
        if n == 0 {
            break;
        }
        let scan_end = carry + n;
        if let Some(caps) = re.captures(&buf[..scan_end]) {
            return Ok(caps.get(1).unwrap().as_bytes().to_vec());
        }
        if scan_end > OVERLAP {
            buf.copy_within(scan_end - OVERLAP..scan_end, 0);
            carry = OVERLAP;
        } else {
            carry = scan_end;
        }
    }
    Err(Error::Store(
        "encryptionKey marker not found in app.asar (launcher updated / key rotated?)".into(),
    ))
}

fn decrypt_store(store_bytes: &[u8], key_b64: &[u8]) -> Result<Vec<u8>> {
    if store_bytes.len() < 17 || store_bytes[16] != b':' {
        return Err(Error::Store("store file does not start with `[16-byte IV] :`".into()));
    }
    let iv = &store_bytes[..16];
    let ct = &store_bytes[17..];

    // electron-store salt = `IV.toString('utf8')` from JS — lossy. Mirror it.
    let salt = String::from_utf8_lossy(iv).into_owned().into_bytes();
    let mut derived = [0u8; 32];
    pbkdf2_hmac::<Sha512>(key_b64, &salt, 10_000, &mut derived);

    let mut buf = ct.to_vec();
    let plaintext = cbc::Decryptor::<Aes256>::new(&derived.into(), iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|e| Error::Store(format!("AES-CBC decrypt failed: {e}")))?;
    let len = plaintext.len();
    buf.truncate(len);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_creds_from_store_shape() {
        let v = json!({
            "session": { "key": "X-Rsi-Token", "value": "sess123" },
            "device":  { "key": "X-RSI-Device", "value": "dev456" },
            "application": { "version": "2.13.3" },
            "library": { "defaults": [ { "gameId": "SC", "channelId": "LIVE", "platformId": "prod" } ] }
        });
        let c = parse_credentials(&v).unwrap();
        assert_eq!(c.session_token, "sess123");
        assert_eq!(c.device_token, "dev456");
        assert_eq!(c.default_channel.as_deref(), Some("LIVE"));
        assert_eq!(c.platform_id.as_deref(), Some("prod"));
        assert_eq!(c.launcher_version.as_deref(), Some("2.13.3"));
    }

    #[test]
    fn missing_session_errors() {
        let v = json!({ "device": { "value": "dev" } });
        assert!(matches!(
            parse_credentials(&v),
            Err(Error::MissingCredential { what: "session token" })
        ));
    }
}
