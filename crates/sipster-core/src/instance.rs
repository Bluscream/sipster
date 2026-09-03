//! Keeping one copy of Sipster running at a time.
//!
//! Two copies is not a cosmetic problem: both would claim the same local SIP
//! port (5060) and fight over credentials/registration with the PBX.
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
pub fn claim() -> io::Result<Option<Guard>> {
    use std::fs::OpenOptions;

    let path = lock_path();
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
/// directory. Either way it is per-user, which is the scope that matters.
pub fn lock_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    dir.join("sipster").join("instance.lock")
}

#[cfg(test)]
mod tests {
    use super::lock_path;

    #[test]
    fn the_lock_lives_under_a_directory_of_our_own() {
        let path = lock_path();
        assert_eq!(path.file_name().unwrap(), "instance.lock");
        assert_eq!(path.parent().unwrap().file_name().unwrap(), "sipster");
    }

    #[test]
    fn a_second_claim_in_this_process_sees_same_path() {
        assert_eq!(lock_path(), lock_path());
    }
}
