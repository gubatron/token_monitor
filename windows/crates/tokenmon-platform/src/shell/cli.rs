//! Command-line arguments of the TokenMon executable.
//!
//! * `--background`: launched at login; start in the tray without opening the panel.
//! * `--show`: bring the panel up (in the running instance if there is one).

use std::ffi::OsString;

use thiserror::Error;

/// Parsed launch arguments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaunchArgs {
    /// `--background` was given.
    pub background: bool,
    /// `--show` was given.
    pub show: bool,
}

/// An argument that is not recognised.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unrecognised argument: {0:?}")]
pub struct LaunchArgsError(pub String);

impl LaunchArgs {
    /// Parses arguments excluding `argv[0]`. Flags are case-insensitive and
    /// may repeat.
    ///
    /// # Errors
    ///
    /// [`LaunchArgsError`] for anything that is not a known flag.
    ///
    /// ```
    /// use tokenmon_platform::shell::LaunchArgs;
    /// let a = LaunchArgs::parse(["--background"]).unwrap();
    /// assert!(a.background && !a.show);
    /// assert!(LaunchArgs::parse(["--bogus"]).is_err());
    /// ```
    pub fn parse<I, S>(args: I) -> Result<Self, LaunchArgsError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        args.into_iter().try_fold(Self::default(), |acc, arg| {
            let arg: OsString = arg.into();
            let text = arg.to_string_lossy();
            match text.to_ascii_lowercase().as_str() {
                "--background" => Ok(Self {
                    background: true,
                    ..acc
                }),
                "--show" => Ok(Self { show: true, ..acc }),
                _ => Err(LaunchArgsError(text.into_owned())),
            }
        })
    }

    /// Like [`parse`](Self::parse) but ignores unknown arguments, so an OS
    /// activation argument can never stop the app from starting.
    #[must_use]
    pub fn parse_lenient<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        args.into_iter().fold(Self::default(), |acc, arg| {
            Self::parse([arg]).map_or(acc, |one| Self {
                background: acc.background || one.background,
                show: acc.show || one.show,
            })
        })
    }

    /// Whether the panel should be visible after startup: asked for with
    /// `--show`, or a normal (non-background) launch.
    #[must_use]
    pub fn wants_panel(&self) -> bool {
        self.show || !self.background
    }
}
