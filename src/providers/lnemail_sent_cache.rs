//! An opt-in, per-account, locally encrypted record of mail actually sent
//! through this app to LNemail — never a general sent archive, and never
//! anything for a message LNemail's own site (or any other client) sent on
//! this mailbox's behalf, since nothing here ever asks LNemail for a body it
//! does not keep. Off unless the caller asks for it on the send itself.
//!
//! Encrypted with the mailbox's own bearer token: every key here is derived
//! from it fresh, nothing derived is itself stored, and a token that has
//! since been rotated verifies nothing and decrypts nothing — a cached
//! message from before a rotation is unreadable after it, by construction,
//! not by a flag this checks.
use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use base64::{Engine, engine::general_purpose::STANDARD};
use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2_hkdf::Sha256;
use std::path::Path;

use crate::platform::private_fs::{atomic_replace, directories, regular, sync_dir};

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

const MAX_ENTRIES: usize = 20;
const MAX_FILE: usize = 256 * 1024;
const MAX_PLAINTEXT: usize = 1024 * 1024;

fn keys(token: &str) -> ([u8; 32], [u8; 32]) {
    let kdf = Hkdf::<Sha256>::new(None, token.as_bytes());
    let mut enc = [0u8; 32];
    let mut mac = [0u8; 32];
    // Fixed 32-byte outputs from a 32-byte-digest HKDF are always in range.
    kdf.expand(b"omamail-lnemail-sent-enc-v1", &mut enc)
        .expect("32-byte expand from HKDF-SHA256 cannot fail");
    kdf.expand(b"omamail-lnemail-sent-mac-v1", &mut mac)
        .expect("32-byte expand from HKDF-SHA256 cannot fail");
    (enc, mac)
}

/// The account id and payment hash are folded into the MAC as associated
/// data, so a ciphertext copied from one entry, or one mailbox, into
/// another's slot fails verification rather than decrypting as if it
/// belonged there.
fn tag(mac_key: &[u8; 32], account: &str, hash: &str, iv: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(mac_key).expect("HMAC accepts any key length");
    mac.update(account.as_bytes());
    mac.update(&[0]);
    mac.update(hash.as_bytes());
    mac.update(&[0]);
    mac.update(iv);
    mac.update(ciphertext);
    mac.finalize().into_bytes().to_vec()
}

fn seal(account: &str, hash: &str, token: &str, plaintext: &[u8]) -> Result<Value, &'static str> {
    let (enc_key, mac_key) = keys(token);
    let mut iv = [0u8; 16];
    getrandom::getrandom(&mut iv).map_err(|_| "lnemail_sent_cache_unavailable")?;
    let ciphertext =
        Aes256CbcEnc::new(&enc_key.into(), &iv.into()).encrypt_padded_vec::<Pkcs7>(plaintext);
    let stamp = tag(&mac_key, account, hash, &iv, &ciphertext);
    Ok(json!({
        "iv": STANDARD.encode(iv),
        "ciphertext": STANDARD.encode(&ciphertext),
        "mac": STANDARD.encode(stamp),
    }))
}

/// `None` covers every reason a row cannot be read back: nothing cached for
/// this hash, a token that no longer matches the one it was sealed with, or
/// a file that failed to parse — all of them mean the same thing to a
/// caller, which is to fall back to LNemail's own status log.
fn open(account: &str, hash: &str, token: &str, entry: &Value) -> Option<Vec<u8>> {
    let iv = STANDARD.decode(entry["iv"].as_str()?).ok()?;
    let ciphertext = STANDARD.decode(entry["ciphertext"].as_str()?).ok()?;
    let stamp = STANDARD.decode(entry["mac"].as_str()?).ok()?;
    if iv.len() != 16 {
        return None;
    }
    let (enc_key, mac_key) = keys(token);
    let expected = tag(&mac_key, account, hash, &iv, &ciphertext);
    if expected.len() != stamp.len() || !constant_time_eq(&expected, &stamp) {
        return None;
    }
    let iv: [u8; 16] = iv.try_into().ok()?;
    Aes256CbcDec::new(&enc_key.into(), &iv.into())
        .decrypt_padded_vec::<Pkcs7>(&ciphertext)
        .ok()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn safe_name(account: &str) -> String {
    let mut out = String::new();
    for byte in account.to_lowercase().bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'@' | b':') {
            out.push(byte as char);
        } else {
            use std::fmt::Write;
            write!(&mut out, "_{byte:02x}").unwrap();
        }
    }
    format!("{out}.json")
}

fn root() -> Result<std::path::PathBuf, &'static str> {
    Ok(crate::platform::dirs::AppDirs::discover()?.cache)
}

fn load(base: &Path, name: &str) -> Result<Value, &'static str> {
    let Some(dir) = directories(base, &["omamail", "lnemail-sent"], false)? else {
        return Ok(json!({"version":1,"entries":{}}));
    };
    let Some(mut file) = regular(&dir, name, false)? else {
        return Ok(json!({"version":1,"entries":{}}));
    };
    use std::io::Read;
    if file.metadata().map_err(|_| "lnemail_sent_cache_unavailable")?.len() > MAX_FILE as u64 {
        return Ok(json!({"version":1,"entries":{}}));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "lnemail_sent_cache_unavailable")?;
    if bytes.len() > MAX_FILE {
        return Ok(json!({"version":1,"entries":{}}));
    }
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if value["version"] == 1 && value["entries"].is_object() {
        Ok(value)
    } else {
        Ok(json!({"version":1,"entries":{}}))
    }
}

/// Encrypts `plaintext` under this account's token and remembers it for
/// `hash` — the same payment hash LNemail's own recent-sends log names the
/// row by, which is what a later read matches it against. Caching is best
/// effort: a failure here must never fail the send it rides along with, so
/// every caller of this treats its own errors as silent.
pub fn remember(account: &str, hash: &str, token: &str, plaintext: &Value) -> Result<(), &'static str> {
    if account.is_empty() || hash.is_empty() || token.is_empty() {
        return Err("invalid_params");
    }
    let bytes = serde_json::to_vec(plaintext).map_err(|_| "lnemail_sent_cache_unavailable")?;
    if bytes.len() > MAX_PLAINTEXT {
        return Err("lnemail_sent_cache_unavailable");
    }
    let base = root()?;
    let name = safe_name(account);
    let mut store = load(&base, &name)?;
    let sealed = seal(account, hash, token, &bytes)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut sealed = sealed;
    sealed["at"] = json!(now);
    store["entries"][hash] = sealed;

    if let Some(entries) = store["entries"].as_object()
        && entries.len() > MAX_ENTRIES
    {
        let mut ordered: Vec<(String, u64)> = entries
            .iter()
            .map(|(key, value)| (key.clone(), value["at"].as_u64().unwrap_or(0)))
            .collect();
        ordered.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        let keep: std::collections::HashSet<_> =
            ordered.into_iter().take(MAX_ENTRIES).map(|(key, _)| key).collect();
        if let Some(entries) = store["entries"].as_object_mut() {
            entries.retain(|key, _| keep.contains(key));
        }
    }

    let dir = directories(&base, &["omamail", "lnemail-sent"], true)?.ok_or("lnemail_sent_cache_unavailable")?;
    let bytes = serde_json::to_vec(&store).map_err(|_| "lnemail_sent_cache_unavailable")?;
    atomic_replace(&dir, &name, &bytes)?;
    sync_dir(&dir)?;
    Ok(())
}

/// Reads back what was cached for `hash`, if anything was and the current
/// token can still open it. `None` either way is the expected, ordinary
/// answer for a message sent from anywhere but this app, or from this app
/// before this feature was on, or under a token since rotated — a caller
/// falls back to LNemail's own status log in every one of those cases alike.
pub fn recall(account: &str, hash: &str, token: &str) -> Option<Value> {
    if account.is_empty() || hash.is_empty() || token.is_empty() {
        return None;
    }
    let base = root().ok()?;
    let name = safe_name(account);
    let store = load(&base, &name).ok()?;
    let entry = store["entries"].get(hash)?;
    let plaintext = open(account, hash, token, entry)?;
    serde_json::from_slice(&plaintext).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn isolated<T>(run: impl FnOnce() -> T) -> T {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let root = std::env::temp_dir().join(format!(
            "omamail-lnemail-sent-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let dirs = crate::platform::dirs::AppDirs::from_roots(
            root.join("config"),
            root.join("cache"),
            root.join("state"),
            root.join("runtime"),
            root.join("downloads"),
        )
        .unwrap();
        let _override =
            crate::platform::dirs::install_test_override(dirs, root.join("home")).unwrap();
        let result = run();
        drop(_override);
        let _ = std::fs::remove_dir_all(&root);
        result
    }

    #[test]
    fn remembered_content_is_recalled_with_the_same_token() {
        isolated(|| {
            let account = "lnemail:sender@example.org";
            remember(account, "hash-1", "token-a", &json!({"to":"you@example.org","subject":"Hi","body":"Hello"}))
                .unwrap();
            let recalled = recall(account, "hash-1", "token-a").unwrap();
            assert_eq!(recalled["subject"], "Hi");
            assert_eq!(recalled["body"], "Hello");
        });
    }

    #[test]
    fn a_rotated_token_cannot_recall_a_previously_sealed_entry() {
        isolated(|| {
            let account = "lnemail:sender@example.org";
            remember(account, "hash-2", "token-old", &json!({"to":"a@example.org","subject":"S","body":"B"}))
                .unwrap();
            assert!(recall(account, "hash-2", "token-new").is_none());
            assert!(recall(account, "hash-2", "token-old").is_some());
        });
    }

    #[test]
    fn an_unknown_hash_or_account_recalls_nothing() {
        isolated(|| {
            let account = "lnemail:sender@example.org";
            remember(account, "hash-3", "token-a", &json!({"to":"a@example.org","subject":"S","body":"B"}))
                .unwrap();
            assert!(recall(account, "hash-does-not-exist", "token-a").is_none());
            assert!(recall("lnemail:other@example.org", "hash-3", "token-a").is_none());
        });
    }

    #[test]
    fn entries_beyond_the_cap_drop_the_oldest_first() {
        isolated(|| {
            let account = "lnemail:sender@example.org";
            for i in 0..(MAX_ENTRIES + 5) {
                remember(
                    account,
                    &format!("hash-{i}"),
                    "token-a",
                    &json!({"to":"a@example.org","subject":format!("S{i}"),"body":"B"}),
                )
                .unwrap();
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert!(recall(account, "hash-0", "token-a").is_none());
            assert!(recall(account, &format!("hash-{}", MAX_ENTRIES + 4), "token-a").is_some());
        });
    }
}
