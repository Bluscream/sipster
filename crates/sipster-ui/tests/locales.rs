//! Lints the translation files.
//!
//! A test rather than a script, so it runs with everything else under
//! `scripts/build.sh check` and cannot be forgotten. It reads the `.yml` files
//! as text: the format is one `key: "value"` per line, which this crate owns,
//! so no YAML dependency is needed to check it.
//!
//! Each check collects every problem before failing, so one run tells you
//! everything rather than one thing at a time.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The locale every other one falls back to. Must be complete.
const FALLBACK: &str = "en";

/// Locales shipped with the app.
const LOCALES: &[&str] = &["en", "de"];

/// What separates a key from its value on a line.
const KEY_VALUE_SEPARATOR: &str = ": ";

fn locales_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("locales")
}

/// One entry as written in the file.
struct Entry {
    line: usize,
    key: String,
    value: String,
}

/// Reads a locale file into entries, in file order.
///
/// Panics rather than returns an error: an unreadable or malformed locale is a
/// broken build, and every caller here would only unwrap anyway.
fn read(locale: &str) -> Vec<Entry> {
    let path = locales_dir().join(format!("{locale}.yml"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    let mut entries = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim_end();
        if trimmed.trim_start().starts_with('#') || trimmed.trim().is_empty() {
            continue;
        }
        assert!(
            !raw.starts_with(' '),
            "{locale}.yml:{line}: indented, so the file is nested again — \
             keys must be flat and dotted"
        );
        let (key, value) = trimmed.split_once(KEY_VALUE_SEPARATOR).unwrap_or_else(|| {
            panic!("{locale}.yml:{line}: no `key: value` separator in {trimmed:?}")
        });
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or_else(|| {
                panic!("{locale}.yml:{line}: value is not double-quoted: {value:?}")
            });
        entries.push(Entry {
            line,
            key: key.trim().to_string(),
            value: value.replace("\\\"", "\"").replace("\\\\", "\\"),
        });
    }
    entries
}

fn map(entries: &[Entry]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|e| (e.key.clone(), e.value.clone()))
        .collect()
}

/// Fails with every problem at once, or passes silently.
fn report(what: &str, problems: &[String]) {
    assert!(
        problems.is_empty(),
        "{what}:\n\n{}\n",
        problems.join("\n")
    );
}

/// Two keys with the same text are usually one key used twice.
///
/// This is what flattening the files was for: while they were nested, the same
/// string could sit under two sections with neither visible from the other.
#[test]
fn no_two_keys_share_a_value() {
    let mut problems = Vec::new();
    for locale in LOCALES {
        let mut by_value: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let entries = read(locale);
        for entry in &entries {
            by_value
                .entry(entry.value.as_str())
                .or_default()
                .push(entry.key.as_str());
        }
        for (value, keys) in by_value {
            if keys.len() > 1 {
                problems.push(format!(
                    "  {locale}.yml: {} have the same value: {value:?} (can possibly be combined)",
                    keys.join(", ")
                ));
            }
        }
    }
    report("translation keys duplicate each other", &problems);
}

/// A key present in the fallback and missing elsewhere shows English in the
/// middle of a translated window.
#[test]
fn every_locale_covers_the_fallback() {
    let fallback: BTreeSet<String> = map(&read(FALLBACK)).into_keys().collect();

    let mut problems = Vec::new();
    for locale in LOCALES.iter().filter(|l| **l != FALLBACK) {
        let present: BTreeSet<String> = map(&read(locale)).into_keys().collect();
        for key in fallback.difference(&present) {
            problems.push(format!("  {locale}.yml: missing {key}"));
        }
        for key in present.difference(&fallback) {
            problems.push(format!(
                "  {locale}.yml: {key} is not in {FALLBACK}.yml, so nothing can reach it"
            ));
        }
    }
    report("locales disagree about which keys exist", &problems);
}

/// An empty value renders as nothing at all — a blank button, a blank label.
#[test]
fn no_value_is_empty() {
    let mut problems = Vec::new();
    for locale in LOCALES {
        for entry in read(locale) {
            if entry.value.trim().is_empty() {
                problems.push(format!(
                    "  {locale}.yml:{}: {} is empty",
                    entry.line, entry.key
                ));
            }
        }
    }
    report("translations are empty", &problems);
}

/// A key written twice silently keeps only the last one.
#[test]
fn no_key_is_defined_twice() {
    let mut problems = Vec::new();
    for locale in LOCALES {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for entry in read(locale) {
            if let Some(first) = seen.insert(entry.key.clone(), entry.line) {
                problems.push(format!(
                    "  {locale}.yml:{}: {} was already defined on line {first}",
                    entry.line, entry.key
                ));
            }
        }
    }
    report("translation keys are defined more than once", &problems);
}

/// Sorted files make a key findable by name, and keep diffs to the line that
/// changed.
#[test]
fn keys_are_sorted() {
    let mut problems = Vec::new();
    for locale in LOCALES {
        let entries = read(locale);
        for pair in entries.windows(2) {
            if pair[1].key < pair[0].key {
                problems.push(format!(
                    "  {locale}.yml:{}: {} comes after {}",
                    pair[1].line, pair[1].key, pair[0].key
                ));
            }
        }
    }
    report("translation keys are out of order", &problems);
}

/// A translation that drops a `%{placeholder}` loses the value it was meant to
/// show; one that invents a placeholder renders it literally.
#[test]
fn placeholders_match_the_fallback() {
    fn placeholders(value: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut rest = value;
        while let Some(start) = rest.find("%{") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            found.insert(rest[..end].trim().to_string());
            rest = &rest[end + 1..];
        }
        found
    }

    let fallback = map(&read(FALLBACK));
    let mut problems = Vec::new();
    for locale in LOCALES.iter().filter(|l| **l != FALLBACK) {
        for entry in read(locale) {
            let Some(original) = fallback.get(&entry.key) else {
                continue; // reported by `every_locale_covers_the_fallback`
            };
            let (want, got) = (placeholders(original), placeholders(&entry.value));
            if want != got {
                problems.push(format!(
                    "  {locale}.yml:{}: {} has placeholders {got:?}, {FALLBACK} has {want:?}",
                    entry.line, entry.key
                ));
            }
        }
    }
    report("placeholders differ from the fallback", &problems);
}

/// A prefix is only worth carrying when it tells two keys apart.
///
/// `add_contact` means the same thing wherever it appears, so writing
/// `history.add_contact` adds length without adding meaning. Where two areas
/// genuinely need different words for the same idea — a contacts count and a
/// call count — the prefix earns its place and stays.
#[test]
fn no_key_carries_a_prefix_it_does_not_need() {
    let keys: Vec<String> = map(&read(FALLBACK)).into_keys().collect();
    let mut by_leaf: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for key in &keys {
        let leaf = key.rsplit('.').next().unwrap_or(key);
        by_leaf.entry(leaf).or_default().push(key);
    }

    let problems: Vec<String> = keys
        .iter()
        .filter(|key| key.contains('.'))
        .filter_map(|key| {
            let leaf = key.rsplit('.').next().unwrap_or(key);
            let shared = by_leaf.get(leaf).map_or(0, Vec::len);
            (shared < 2).then(|| format!("  {key} could just be {leaf}"))
        })
        .collect();
    report("keys carry prefixes that distinguish nothing", &problems);
}

/// A key nothing asks for is dead weight every future translation carries; a
/// key the code asks for and the fallback lacks renders as the key itself.
#[test]
fn keys_and_code_agree() {
    let used = used_keys();
    let defined: BTreeSet<String> = map(&read(FALLBACK)).into_keys().collect();

    let mut problems: Vec<String> = used
        .difference(&defined)
        .map(|key| format!("  {key} is used in code but missing from {FALLBACK}.yml"))
        .collect();
    problems.extend(
        defined
            .difference(&used)
            .map(|key| format!("  {key} is defined but nothing uses it")),
    );
    report("the locales and the code disagree", &problems);
}

/// Every key named by a `t!("…")` call under `src/`.
///
/// Text scanning rather than parsing: all call sites pass a literal, which the
/// `keys_and_code_agree` test would catch changing, because a computed key
/// would show up as an unused definition.
fn used_keys() -> BTreeSet<String> {
    fn walk(dir: &Path, found: &mut BTreeSet<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                collect(&text, found);
            }
        }
    }

    /// Pulls the literal out of every `t!("key"` in `text`.
    fn collect(text: &str, found: &mut BTreeSet<String>) {
        let mut rest = text;
        while let Some(at) = rest.find("t!(") {
            rest = &rest[at + 3..];
            let after = rest.trim_start();
            let Some(body) = after.strip_prefix('"') else {
                continue;
            };
            let Some(end) = body.find('"') else { break };
            let key = &body[..end];
            if !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_')
            {
                found.insert(key.to_string());
            }
        }
    }

    let mut found = BTreeSet::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
    assert!(
        !found.is_empty(),
        "found no t! calls at all — the scan is broken, not the locales"
    );
    found
}
