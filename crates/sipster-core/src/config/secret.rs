//! Encryption for credentials stored in the config file.
//!
//! Passwords, tokens, usernames and email addresses are encrypted on the way
//! into `sipster.toml` and decrypted on the way out, so the file holds no
//! readable credentials.
//!
//! # What this protects against, and what it does not
//!
//! It stops credentials being legible to a glance over a shoulder, a screen
//! share, a backup that gets copied somewhere careless, or a `grep` for
//! something else that scrolls the file past. It does **not** protect against
//! someone who can read the user's home directory: the key sits next to the
//! config, because a softphone has to be able to register on its own without
//! prompting for a passphrase. What actually guards both files is their `0600`
//! mode. A real answer would be the desktop keyring, which is a bigger change
//! and can fail on a headless machine.
//!
//! # Why authenticated encryption, and why no marker
//!
//! Values are written as bare base64 with no prefix, so a config that has
//! never been encrypted still loads. That works because ChaCha20-Poly1305 is
//! authenticated: decryption verifies a 128-bit tag, so a plain-text value
//! that happens to be valid base64 fails the check and is passed through
//! untouched. The chance of a real password authenticating by accident is
//! about one in 2^128.
//!
//! An earlier attempt at plain base64 needed a `b64:` prefix precisely because
//! it had no such check — `sysadmin` is valid base64 and decodes to four bytes
//! of nonsense, so unprefixed decoding corrupted exactly the credentials it
//! was meant to protect.

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use base64::Engine as _;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Deserializer, Serializer};

/// Where the key lives, set from the config path before load or save.
static KEY_PATH: RwLock<Option<PathBuf>> = RwLock::new(None);

/// The key itself, read or created once.
static KEY: OnceLock<Option<[u8; 32]>> = OnceLock::new();

/// Points the codec at the key beside `config_path`.
///
/// Called before reading or writing the config, so `--config-file` keeps its
/// key next to the file it belongs to rather than sharing one globally.
pub fn use_key_beside(config_path: &Path) {
    let path = config_path.with_extension("key");
    if let Ok(mut slot) = KEY_PATH.write() {
        *slot = Some(path);
    }
}

/// The key, loading or creating it on first use.
///
/// `None` when no key could be read or written — a read-only config directory,
/// say. Credentials are then stored as they always were, in the clear, because
/// refusing to save at all would be the worse failure.
fn key() -> Option<&'static [u8; 32]> {
    KEY.get_or_init(|| {
        let path = KEY_PATH.read().ok()?.clone()?;
        match load_or_create_key(&path) {
            Ok(key) => Some(key),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "no credential key; credentials will be stored in the clear"
                );
                None
            }
        }
    })
    .as_ref()
}

fn load_or_create_key(path: &Path) -> std::io::Result<[u8; 32]> {
    if let Ok(bytes) = std::fs::read(path) {
        if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(key);
        }
        // A truncated or corrupt key cannot be recovered, and replacing it
        // would silently orphan every stored credential. Say so instead.
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "key file is not 32 bytes",
        ));
    }

    let mut key = [0u8; 32];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| std::io::Error::other("no system randomness"))?;

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_private(path, &key)?;
    Ok(key)
}

/// Writes `bytes` readable only by this user.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

/// Encrypts a credential for storage, or returns it unchanged when there is no
/// key to encrypt it with.
#[must_use]
pub fn encode(plain: &str) -> String {
    if plain.is_empty() {
        // An empty field means "unset"; encrypting it would make an absent
        // password indistinguishable from a stored one.
        return String::new();
    }
    let Some(key) = key() else {
        return plain.to_owned();
    };
    seal(key, plain).unwrap_or_else(|| plain.to_owned())
}

fn seal(key: &[u8; 32], plain: &str) -> Option<String> {
    let unbound = UnboundKey::new(&CHACHA20_POLY1305, key).ok()?;
    let sealing = LessSafeKey::new(unbound);

    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).ok()?;

    let mut buffer = plain.as_bytes().to_vec();
    sealing
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buffer)
        .ok()?;

    // The nonce is not a secret and has to travel with the ciphertext.
    let mut out = nonce.to_vec();
    out.extend_from_slice(&buffer);
    Some(base64::engine::general_purpose::STANDARD.encode(out))
}

/// Decrypts a credential, passing anything that is not our ciphertext through
/// unchanged — which is how a config written before this existed still loads.
#[must_use]
pub fn decode(stored: &str) -> String {
    if stored.is_empty() {
        return String::new();
    }
    key()
        .and_then(|key| open(key, stored))
        .unwrap_or_else(|| stored.to_owned())
}

fn open(key: &[u8; 32], stored: &str) -> Option<String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(stored).ok()?;
    if raw.len() <= NONCE_LEN {
        return None;
    }
    let (nonce, body) = raw.split_at(NONCE_LEN);
    let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;

    let unbound = UnboundKey::new(&CHACHA20_POLY1305, key).ok()?;
    let opening = LessSafeKey::new(unbound);

    let mut buffer = body.to_vec();
    // Fails the tag check for anything that is not our ciphertext, which is
    // exactly the test that lets plain text pass through untouched.
    let plain = opening.open_in_place(nonce, Aad::empty(), &mut buffer).ok()?;
    String::from_utf8(plain.to_vec()).ok()
}

/// `serde` glue, used as `#[serde(with = "secret")]` on a `String` field.
pub fn serialize<S: Serializer>(plain: &str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&encode(plain))
}

/// See [`serialize`].
///
/// # Errors
///
/// Only when the field is not a string at all.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let stored = String::deserialize(deserializer)?;
    Ok(decode(&stored))
}

/// The same, for `Option<String>` fields.
pub mod optional {
    use serde::{Deserialize, Deserializer, Serializer};

    /// See [`super::serialize`].
    pub fn serialize<S: Serializer>(
        plain: &Option<String>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match plain {
            Some(value) => serializer.serialize_some(&super::encode(value)),
            None => serializer.serialize_none(),
        }
    }

    /// See [`super::deserialize`].
    ///
    /// # Errors
    ///
    /// Only when the field is not a string at all.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error> {
        Ok(Option::<String>::deserialize(deserializer)?.map(|stored| super::decode(&stored)))
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, use_key_beside};

    /// The key is process-global and created once, so every test shares one
    /// scratch directory rather than fighting over it.
    fn with_key() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let dir = std::env::temp_dir().join(format!("sipster-secret-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("scratch dir");
            use_key_beside(&dir.join("sipster.toml"));
        });
    }

    #[test]
    fn a_credential_round_trips_and_is_not_readable_in_the_file() {
        with_key();
        for plain in ["hunter2", "s3cr3t!", "üñïçødé", "with spaces and = signs"] {
            let stored = encode(plain);
            assert!(!stored.contains(plain), "{plain} must not appear in the file");
            assert_eq!(decode(&stored), plain);
        }
    }

    /// A config written before this existed holds plain text and must keep
    /// working — an account that silently stopped authenticating on upgrade
    /// would be a miserable thing to debug.
    #[test]
    fn plain_text_from_an_older_config_is_read_unchanged() {
        with_key();
        for plain in ["hunter2", "bluscream", "user@example.com"] {
            assert_eq!(decode(plain), plain);
        }
    }

    /// The reason a marker is not needed: the Poly1305 tag rejects anything
    /// that is not our ciphertext, including a password that is itself valid
    /// base64. Plain base64 had no such check and needed a `b64:` prefix.
    #[test]
    fn a_password_that_is_valid_base64_is_not_mangled() {
        with_key();
        assert_eq!(decode("sysadmin"), "sysadmin");
        assert_eq!(decode("YWJjZA=="), "YWJjZA==");
        assert_eq!(decode("cGFzc3dvcmQxMjM0NTY3OA=="), "cGFzc3dvcmQxMjM0NTY3OA==");
    }

    /// Every value gets its own nonce, so the same password twice does not
    /// produce the same ciphertext — otherwise the file would leak which
    /// accounts share a password.
    #[test]
    fn the_same_secret_encrypts_differently_each_time() {
        with_key();
        let (a, b) = (encode("hunter2"), encode("hunter2"));
        assert_ne!(a, b, "nonce must be per-value");
        assert_eq!(decode(&a), "hunter2");
        assert_eq!(decode(&b), "hunter2");
    }

    /// An unset field must stay unset rather than becoming a stored empty
    /// secret.
    #[test]
    fn an_empty_field_stays_empty() {
        with_key();
        assert_eq!(encode(""), "");
        assert_eq!(decode(""), "");
    }
}
