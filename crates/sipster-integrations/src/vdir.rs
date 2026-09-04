//! Contacts from a directory of `.vcf` files.
//!
//! Linux has no single system contact store. What it does have is a widely
//! shared *file* convention — one vCard per file in a directory — used by
//! `vdirsyncer`, Radicale, Baïkal, khard, and KDE's directory-backed address
//! books. Reading it costs no D-Bus, no desktop-specific service and no extra
//! dependency, and it is the interop point everything else can already write
//! to.
//!
//! Evolution Data Server and Akonadi are the other two obvious sources. Both
//! need D-Bus clients against services that are not present on every machine,
//! so neither is implemented here rather than shipped unverified — see the
//! README for the current state.

use std::path::{Path, PathBuf};

use crate::model::{Contact, RecordSource};
use crate::vcard;

/// Directories searched when none is configured, in order of preference.
///
/// `~/.local/share/contacts` is the XDG-ish location vdirsyncer and KDE use;
/// `~/.contacts` is the older convention some tools still write.
const DEFAULT_DIRS: [&str; 2] = [".local/share/contacts", ".contacts"];

/// A directory of vCards.
#[derive(Debug, Clone)]
pub struct VdirStore {
    root: PathBuf,
}

impl VdirStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Every vCard directory on this machine worth reading.
    ///
    /// The conventional locations, plus each directory Akonadi is configured
    /// with — a KDE user's address books are ordinary vCard directories and
    /// need nothing Akonadi-specific to read. See [`crate::akonadi`].
    #[must_use]
    pub fn discover() -> Vec<Self> {
        let mut roots: Vec<PathBuf> = std::env::var_os("HOME")
            .map(|home| {
                DEFAULT_DIRS
                    .iter()
                    .map(|relative| Path::new(&home).join(relative))
                    .filter(|path| path.is_dir())
                    .collect()
            })
            .unwrap_or_default();
        roots.extend(crate::akonadi::contact_directories());

        // Akonadi's default resource points at `~/.local/share/contacts`, so
        // without this the conventional directory is read twice.
        roots.sort();
        roots.dedup();
        roots.into_iter().map(Self::new).collect()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads every vCard under the directory.
    ///
    /// vdir puts one card per file, but collections nest one level (a
    /// directory per address book), so both layouts are walked. A file that
    /// cannot be read or parsed is skipped rather than failing the sync — one
    /// bad card should not cost the user their whole address book.
    ///
    /// # Errors
    ///
    /// Only when the root directory itself cannot be listed.
    pub fn load(&self) -> std::io::Result<Vec<Contact>> {
        let mut contacts = Vec::new();
        self.collect(&self.root, 0, &mut contacts)?;
        Ok(contacts)
    }

    fn collect(&self, dir: &Path, depth: u32, out: &mut Vec<Contact>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();

            if path.is_dir() {
                // One level of collections; deeper is not a vdir layout and
                // guards against a symlink loop walking the filesystem.
                if depth == 0 {
                    let _ = self.collect(&path, depth + 1, out);
                }
                continue;
            }

            if !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("vcf"))
            {
                continue;
            }

            let Ok(text) = std::fs::read_to_string(&path) else {
                tracing::warn!(path = %path.display(), "could not read vCard");
                continue;
            };

            let book = collection_name(&self.root, &path);
            for card in vcard::split_cards(&text) {
                if let Some(contact) = vcard::parse(
                    card,
                    "vdir",
                    RecordSource::Other(format!("Local vCards ({book})")),
                ) {
                    out.push(contact);
                }
            }
        }
        Ok(())
    }
}

/// The address book a file belongs to: its parent directory, or the root's
/// own name for files sitting directly in it.
fn collection_name(root: &Path, file: &Path) -> String {
    file.parent()
        .filter(|parent| *parent != root)
        .or(Some(root))
        .and_then(|dir| dir.file_name())
        .map_or_else(|| "contacts".to_string(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::VdirStore;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sipster-vdir-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &std::path::Path, file: &str, body: &str) {
        if let Some(parent) = dir.join(file).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            dir.join(file),
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\n{body}\r\nEND:VCARD\r\n"),
        )
        .unwrap();
    }

    #[test]
    fn reads_one_card_per_file() {
        let dir = scratch("flat");
        write(&dir, "a.vcf", "UID:1\r\nFN:Alice\r\nTEL:+4930111");
        write(&dir, "b.vcf", "UID:2\r\nFN:Bob\r\nTEL:+4930222");

        let mut contacts = VdirStore::new(dir.clone()).load().unwrap();
        contacts.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(contacts.len(), 2);
        assert_eq!(contacts[0].name, "Alice");
        assert_eq!(contacts[1].numbers[0].number, "+4930222");
        std::fs::remove_dir_all(dir).ok();
    }

    /// vdirsyncer stores one directory per address book.
    #[test]
    fn walks_one_level_of_collections() {
        let dir = scratch("nested");
        write(&dir, "work/a.vcf", "UID:1\r\nFN:Alice\r\nTEL:1");
        write(&dir, "personal/b.vcf", "UID:2\r\nFN:Bob\r\nTEL:2");

        let contacts = VdirStore::new(dir.clone()).load().unwrap();
        assert_eq!(contacts.len(), 2);
        let books: Vec<String> = contacts.iter().map(|c| c.source.to_string()).collect();
        assert!(books.iter().any(|b| b.contains("work")));
        assert!(books.iter().any(|b| b.contains("personal")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn non_vcf_files_are_ignored() {
        let dir = scratch("mixed");
        write(&dir, "a.vcf", "UID:1\r\nFN:Alice\r\nTEL:1");
        std::fs::write(dir.join("notes.txt"), "not a vcard").unwrap();
        std::fs::write(dir.join(".hidden"), "also not").unwrap();

        assert_eq!(VdirStore::new(dir.clone()).load().unwrap().len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    /// One unreadable or malformed card must not cost the whole address book.
    #[test]
    fn a_bad_card_does_not_lose_the_others() {
        let dir = scratch("bad");
        write(&dir, "good.vcf", "UID:1\r\nFN:Alice\r\nTEL:1");
        std::fs::write(dir.join("bad.vcf"), "BEGIN:VCARD\r\ngarbage").unwrap();
        std::fs::write(dir.join("empty.vcf"), "").unwrap();

        assert_eq!(VdirStore::new(dir.clone()).load().unwrap().len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn several_cards_in_one_file_are_all_read() {
        let dir = scratch("multi");
        std::fs::write(
            dir.join("all.vcf"),
            "BEGIN:VCARD\r\nFN:A\r\nTEL:1\r\nEND:VCARD\r\n\
             BEGIN:VCARD\r\nFN:B\r\nTEL:2\r\nEND:VCARD\r\n",
        )
        .unwrap();

        assert_eq!(VdirStore::new(dir.clone()).load().unwrap().len(), 2);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_missing_directory_is_an_error_not_a_panic() {
        assert!(VdirStore::new("/nonexistent/contacts".into()).load().is_err());
    }
}
