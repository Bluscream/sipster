//! Keeping one copy of Sipster running per configuration.
//!
//! Two copies of the *same* config is not a cosmetic problem: both would claim
//! the same local SIP port and fight over credentials and registration with
//! the PBX.
//!
//! Two copies of *different* configs is the supported way to run a second
//! line, so the lock is per config file rather than per user. It used to be
//! per user, which meant a second line could only be started by overriding
//! `XDG_RUNTIME_DIR` — and that breaks the Wayland connection, because
//! `WAYLAND_DISPLAY` is resolved relative to it. The window never appeared.
//!
//! Held by an advisory lock on a file in the runtime directory. A lock is
//! released by the kernel when the process ends, however it ends, so a crash
//! cannot leave a stale lock behind claiming Sipster is already running.

use std::fs::{File, TryLockError};
use std::io;
use std::path::PathBuf;

/// A held claim. Dropping it, or exiting, releases the kernel lock.
#[derive(Debug)]
pub struct Guard {
    _file: File,
}

/// Try to claim the single-instance lock.
///
/// `Ok(Some(Guard))` means this process is the primary instance.
/// `Ok(None)` means another copy already holds it.
///
/// # Errors
///
/// Returns an error only if creating or accessing the lock file itself failed.
pub fn claim(config: &std::path::Path) -> io::Result<Option<Guard>> {
    use std::fs::OpenOptions;

    let path = lock_path_for(config);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;

    match file.try_lock() {
        Ok(()) => Ok(Some(Guard { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

/// The runtime directory when there is one, falling back to the temporary
/// directory. Either way it is per-user, which is the outer scope that matters.
pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from)
}

/// The lock file for the copy running `config`.
///
/// Named after the config rather than the user, so a second config is a second
/// instance. The name is a digest because a config path can be long, contain
/// separators, or sit on a filesystem with different rules than the runtime
/// directory.
pub fn lock_path_for(config: &std::path::Path) -> PathBuf {
    runtime_dir()
        .join("sipster")
        .join(format!("{}.lock", key_for(config)))
}

/// A short, stable, filename-safe digest of a config path.
///
/// Absolute where possible, so the same file reached by a relative and an
/// absolute path is still one instance.
pub fn key_for(config: &std::path::Path) -> String {
    let path = std::fs::canonicalize(config)
        .or_else(|_| std::path::absolute(config))
        .unwrap_or_else(|_| config.to_path_buf());

    // FNV-1a: no dependency, and this only has to avoid accidental collisions
    // between a handful of paths, not resist an adversary.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::{key_for, lock_path_for};
    use std::path::Path;

    #[test]
    fn the_lock_lives_under_a_directory_of_our_own() {
        let path = lock_path_for(Path::new("/tmp/sipster.toml"));
        assert_eq!(path.parent().unwrap().file_name().unwrap(), "sipster");
        assert_eq!(path.extension().unwrap(), "lock");
    }

    #[test]
    fn one_config_is_one_instance() {
        assert_eq!(
            lock_path_for(Path::new("/tmp/sipster.toml")),
            lock_path_for(Path::new("/tmp/sipster.toml"))
        );
    }

    /// The whole point: a second config must be a second instance, so that a
    /// second line does not need `XDG_RUNTIME_DIR` overridden — which breaks
    /// the Wayland connection and leaves the app with no window.
    #[test]
    fn a_different_config_is_a_different_instance() {
        assert_ne!(
            lock_path_for(Path::new("/tmp/one.toml")),
            lock_path_for(Path::new("/tmp/two.toml"))
        );
    }

    #[test]
    fn the_key_is_filename_safe() {
        let key = key_for(Path::new("/tmp/a b/../sipster.toml"));
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()), "{key}");
    }
}
