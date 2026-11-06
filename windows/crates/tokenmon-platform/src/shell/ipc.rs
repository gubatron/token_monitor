//! "Show the panel" message between a second launch and the running instance.
//!
//! Wire format: one ASCII line, `TM1 show\n`. The reader takes at most
//! [`MAX_MESSAGE_LEN`] bytes, so a hostile or buggy peer cannot make the
//! running instance buffer unbounded data. Transport (a named pipe on Windows)
//! is behind [`IpcClient`]; parsing is portable and fuzzed.

use std::io::Read;
use std::sync::{Arc, Mutex};

use thiserror::Error;

use crate::shell::error::ShellError;

/// Upper bound on bytes read from one connection.
pub const MAX_MESSAGE_LEN: usize = 64;

/// A message sent to the running instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpcMessage {
    /// Bring the panel to the front.
    Show,
}

/// Why a message could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IpcParseError {
    /// More than [`MAX_MESSAGE_LEN`] bytes, or no terminating newline within the bound.
    #[error("message too long")]
    TooLong,
    /// Not valid UTF-8 / not a known message.
    #[error("unknown message")]
    Unknown,
}

impl IpcMessage {
    /// Encodes to the wire format.
    ///
    /// ```
    /// use tokenmon_platform::shell::IpcMessage;
    /// assert_eq!(IpcMessage::Show.encode(), b"TM1 show\n");
    /// ```
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Show => b"TM1 show\n".to_vec(),
        }
    }

    /// Decodes one message (a trailing `\n` or `\r\n` is optional).
    ///
    /// # Errors
    ///
    /// [`IpcParseError`] when too long or not a known message.
    pub fn decode(bytes: &[u8]) -> Result<Self, IpcParseError> {
        if bytes.len() > MAX_MESSAGE_LEN {
            return Err(IpcParseError::TooLong);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| IpcParseError::Unknown)?;
        match text.trim_end_matches(['\r', '\n']) {
            "TM1 show" => Ok(Self::Show),
            _ => Err(IpcParseError::Unknown),
        }
    }

    /// Reads at most [`MAX_MESSAGE_LEN`] bytes from `reader` and decodes them.
    /// Used by the server for each accepted connection.
    ///
    /// # Errors
    ///
    /// [`IpcParseError::TooLong`] when the peer sends more than the bound
    /// (read errors count as [`IpcParseError::Unknown`]).
    pub fn read_from(reader: impl Read) -> Result<Self, IpcParseError> {
        let mut buf = Vec::with_capacity(MAX_MESSAGE_LEN + 1);
        reader
            .take(MAX_MESSAGE_LEN as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|_| IpcParseError::Unknown)?;
        Self::decode(&buf)
    }
}

/// Sends a message to the running instance.
pub trait IpcClient: Send + Sync {
    /// Delivers `message`; implementations retry briefly while the primary is
    /// still starting.
    ///
    /// # Errors
    ///
    /// [`ShellError::Ipc`] when the primary cannot be reached.
    fn send(&self, message: &IpcMessage) -> Result<(), ShellError>;
}

/// In-memory IPC for tests: the "server" is the shared inbox.
#[derive(Debug, Clone)]
pub struct FakeIpc {
    inbox: Arc<Mutex<Vec<IpcMessage>>>,
    reachable: bool,
}

impl Default for FakeIpc {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeIpc {
    /// A reachable fake with an empty inbox.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inbox: Arc::default(),
            reachable: true,
        }
    }

    /// A fake whose `send` always fails.
    #[must_use]
    pub fn unreachable() -> Self {
        Self {
            reachable: false,
            ..Self::new()
        }
    }

    /// Messages delivered so far.
    #[must_use]
    pub fn received(&self) -> Vec<IpcMessage> {
        self.inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl IpcClient for FakeIpc {
    fn send(&self, message: &IpcMessage) -> Result<(), ShellError> {
        if !self.reachable {
            return Err(ShellError::Ipc("fake unreachable".into()));
        }
        self.inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(*message);
        Ok(())
    }
}

/// Pipe name for `user`: `\\.\pipe\TokenMon.<user>`. Pipe names are
/// machine-global, so the user name keeps two logged-in users from colliding.
/// Characters outside `[A-Za-z0-9_.-]` become `_`.
///
/// ```
/// use tokenmon_platform::shell::ipc::pipe_name;
/// assert_eq!(pipe_name("Ada Lovelace"), r"\\.\pipe\TokenMon.Ada_Lovelace");
/// ```
#[must_use]
pub fn pipe_name(user: &str) -> String {
    let safe: String = user
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(r"\\.\pipe\TokenMon.{safe}")
}
