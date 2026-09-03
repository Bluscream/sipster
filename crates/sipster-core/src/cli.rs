//! Minimal command-line flag lookup shared by the engine and every frontend.
//!
//! Sipster's CLI surface is tiny — a handful of flags plus a `tel:`/`sip:` URI
//! — and a full argument-parser dependency would outweigh it. What this module
//! exists to prevent is the *third* hand-rolled `while let Some(arg)` loop:
//! [`ipc::socket_path`](crate::ipc::socket_path), the command parser and the
//! UI's `--log-file` handling each grew their own, and they disagreed about
//! whether `--flag=value` was supported.
//!
//! Both spellings are always accepted: `--name value` and `--name=value`.

/// Returns the value of the first flag in `names` that carries one.
///
/// `names` are matched verbatim, so pass them with their dashes
/// (`["--log-file", "-l"]`). An empty or whitespace-only value is treated as
/// absent, which keeps `--log-file ""` from silently disabling logging.
pub fn flag_value<'a, S: AsRef<str>>(args: &'a [S], names: &[&str]) -> Option<&'a str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let arg = arg.as_ref();
        if names.contains(&arg) {
            if let Some(value) = iter.next() {
                let value = value.as_ref().trim();
                if !value.is_empty() {
                    return Some(value);
                }
            }
        } else if let Some(value) = names
            .iter()
            .find_map(|name| arg.strip_prefix(name)?.strip_prefix('='))
        {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// Whether any of `names` appears as a bare flag.
pub fn has_flag<S: AsRef<str>>(args: &[S], names: &[&str]) -> bool {
    args.iter().any(|arg| names.contains(&arg.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::{flag_value, has_flag};

    #[test]
    fn reads_separated_and_joined_spellings() {
        assert_eq!(flag_value(&["--log-file", "/tmp/a.log"], &["--log-file"]), Some("/tmp/a.log"));
        assert_eq!(flag_value(&["--log-file=/tmp/a.log"], &["--log-file"]), Some("/tmp/a.log"));
    }

    #[test]
    fn accepts_any_of_several_names() {
        let names = ["--socket", "-s"];
        assert_eq!(flag_value(&["-s", "/run/x.sock"], &names), Some("/run/x.sock"));
        assert_eq!(flag_value(&["--socket=/run/x.sock"], &names), Some("/run/x.sock"));
    }

    /// A flag whose value is missing or blank must read as absent rather than
    /// yielding an empty path that later fails deep inside an `open()`.
    #[test]
    fn blank_and_missing_values_are_absent() {
        assert_eq!(flag_value(&["--log-file"], &["--log-file"]), None);
        assert_eq!(flag_value(&["--log-file", "  "], &["--log-file"]), None);
        assert_eq!(flag_value(&["--log-file="], &["--log-file"]), None);
    }

    #[test]
    fn finds_the_flag_among_other_arguments() {
        let args = ["--call", "611", "--log-file", "/tmp/a.log", "--show"];
        assert_eq!(flag_value(&args, &["--log-file"]), Some("/tmp/a.log"));
        assert!(has_flag(&args, &["--show"]));
        assert!(!has_flag(&args, &["--quit"]));
    }

    /// `--log-file-name` must not be mistaken for `--log-file`.
    #[test]
    fn does_not_match_a_longer_flag_by_prefix() {
        assert_eq!(flag_value(&["--log-file-name", "x"], &["--log-file"]), None);
        assert!(!has_flag(&["--show-all"], &["--show"]));
    }

    #[test]
    fn returns_the_first_occurrence() {
        assert_eq!(flag_value(&["--socket", "a", "--socket", "b"], &["--socket"]), Some("a"));
    }
}
