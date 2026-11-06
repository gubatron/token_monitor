//! Building and splitting the command lines stored in the registry.

use crate::shell::error::ShellError;

/// Quotes an executable path for a command line.
///
/// The path is always quoted so that `C:\Program Files\TokenMon\TokenMon.exe`
/// is not split at the space (an unquoted path is also a classic
/// unquoted-path hijack). Windows file names cannot contain `"`, so a path
/// that does is rejected instead of escaped.
///
/// # Errors
///
/// [`ShellError::InvalidExecutablePath`] for an empty path or one containing
/// `"`, NUL, CR or LF.
pub fn quote_exe(exe: &str) -> Result<String, ShellError> {
    if exe.is_empty() || exe.contains(['"', '\0', '\r', '\n']) {
        return Err(ShellError::InvalidExecutablePath(exe.to_owned()));
    }
    Ok(format!("\"{exe}\""))
}

/// Builds `"<exe>" <arg> <arg>...` from an executable path and raw flags.
///
/// Flags are fixed literals chosen by this crate (never user input), so they
/// are appended verbatim.
///
/// # Errors
///
/// See [`quote_exe`].
pub fn build(exe: &str, args: &[&str]) -> Result<String, ShellError> {
    let mut line = quote_exe(exe)?;
    for arg in args {
        line.push(' ');
        line.push_str(arg);
    }
    Ok(line)
}

/// Extracts the executable path from a stored command line: the quoted prefix,
/// or the first whitespace-delimited token when unquoted. `None` when empty or
/// the opening quote is never closed.
///
/// ```
/// use tokenmon_platform::shell::command_line::executable;
/// assert_eq!(
///     executable(r#""C:\Program Files\TokenMon\TokenMon.exe" --background"#),
///     Some(r"C:\Program Files\TokenMon\TokenMon.exe")
/// );
/// ```
#[must_use]
pub fn executable(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let exe = match line.strip_prefix('"') {
        Some(rest) => rest.split_once('"')?.0,
        None => line.split_whitespace().next()?,
    };
    (!exe.is_empty()).then_some(exe)
}
