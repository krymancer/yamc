//! Account passwords are sealed with AES-256-GCM. The 32-byte key lives in the OS keychain
//! (macOS Keychain, Secret Service, Windows Credential Manager) or, when none is reachable,
//! in `secret.key` (mode 0600) in the data directory.

use std::path::Path;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};

const SERVICE: &str = "yamc";
const KEY_USER: &str = "master-key";
const VERSION: u8 = 1;

#[derive(Clone)]
pub struct SecretBox {
    cipher: Aes256Gcm,
    /// Where the key came from, for the settings screen.
    pub source: &'static str,
}

pub fn random_token() -> String {
    let mut b = [0u8; 24];
    rand::fill(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

fn new_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    rand::fill(&mut k);
    k
}

fn decode_key(s: &str) -> Option<[u8; 32]> {
    STANDARD.decode(s.trim()).ok()?.try_into().ok()
}

enum Keychain {
    Key([u8; 32]),
    /// No keychain on this system, or it has no key yet and won't store one.
    Unavailable,
    /// Reading failed (access denied, prompt dismissed, no keychain service).
    Refused(String),
}

fn keychain_key() -> Keychain {
    let Ok(entry) = keyring::Entry::new(SERVICE, KEY_USER) else { return Keychain::Unavailable };
    match entry.get_password() {
        Ok(s) => decode_key(&s).map(Keychain::Key).unwrap_or_else(|| Keychain::Refused("the stored key is malformed".into())),
        Err(keyring::Error::NoEntry) => {
            let k = new_key();
            if entry.set_password(&STANDARD.encode(k)).is_err() {
                return Keychain::Unavailable;
            }
            // Read back: some stores accept writes they can't return.
            match entry.get_password().ok().and_then(|s| decode_key(&s)) {
                Some(r) if r == k => Keychain::Key(k),
                _ => Keychain::Unavailable,
            }
        }
        Err(e) => Keychain::Refused(e.to_string()),
    }
}

fn file_key(dir: &Path) -> Result<[u8; 32]> {
    let path = dir.join("secret.key");
    if let Ok(s) = std::fs::read_to_string(&path) {
        return decode_key(&s).ok_or_else(|| anyhow!("{} is corrupt", path.display()));
    }
    let k = new_key();
    std::fs::write(&path, STANDARD.encode(k))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(k)
}

impl SecretBox {
    /// Use the key file if one exists (it was chosen before), else the keychain, else a new key file.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let use_keychain = std::env::var_os("YAMC_NO_KEYCHAIN").is_none() && !data_dir.join("secret.key").exists();
        if use_keychain {
            match keychain_key() {
                Keychain::Key(k) => return Ok(Self::from_key(&k, "system keychain")),
                Keychain::Unavailable => {}
                // Without a mailbox yet there is nothing to lose, so a key file is fine.
                Keychain::Refused(_) if !data_dir.join("mail.db").exists() => {}
                // Otherwise a fresh key file would lock every saved password out for good,
                // since the key file wins from then on.
                Keychain::Refused(e) => {
                    return Err(anyhow!(
                        "can't read yamc's key from the system keychain ({e}). Allow access when macOS asks, then start yamc again"
                    ));
                }
            }
        }
        Ok(Self::from_key(&file_key(data_dir)?, "key file"))
    }

    pub fn from_key(key: &[u8; 32], source: &'static str) -> Self {
        Self { cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key)), source }
    }

    pub fn encrypt(&self, plain: &str) -> Result<Vec<u8>> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ct = self.cipher.encrypt(&nonce, plain.as_bytes()).map_err(|_| anyhow!("encrypt failed"))?;
        let mut out = Vec::with_capacity(1 + nonce.len() + ct.len());
        out.push(VERSION);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    pub fn decrypt(&self, blob: &[u8]) -> Result<String> {
        if blob.len() < 13 || blob[0] != VERSION {
            return Err(anyhow!("unknown secret format; re-add the account"));
        }
        let nonce = Nonce::from_slice(&blob[1..13]);
        let plain = self
            .cipher
            .decrypt(nonce, &blob[13..])
            .map_err(|_| anyhow!("can't decrypt the saved password (key changed?); re-add the account"))?;
        Ok(String::from_utf8(plain)?)
    }
}
