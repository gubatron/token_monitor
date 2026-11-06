//! "Show the panel" message between a second launch and the running instance.
//!
//! Wire format: one ASCII line, `TM1 show\n`. The reader takes at most
//! [`MAX_MESSAGE_LEN`] bytes, so a hostile or buggy peer cannot make the
//! running instance buffer unbounded data. Transport (a named pipe on Windows)
//! is behind [`IpcClient`]; parsing is portable and fuzzed.

use std::io::{self, Read};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::shell::error::ShellError;

/// Upper bound on bytes read from one connection.
pub const MAX_MESSAGE_LEN: usize = 64;

/// Total time one connection may take to deliver its message. A client that
/// connects and stalls (or trickles bytes) is dropped after this, so it can
/// never block the single server thread or hang shutdown.
pub const READ_DEADLINE: Duration = Duration::from_secs(2);

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
    /// The peer did not deliver a complete message before the deadline.
    #[error("timed out waiting for the message")]
    TimedOut,
}

/// A byte source whose every read is bounded by a timeout (the overlapped
/// named-pipe reader on Windows, scripted fakes in tests).
pub trait DeadlineRead {
    /// Reads up to `buf.len()` bytes, waiting at most `timeout`. `Ok(0)` is
    /// end of stream (the peer closed).
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::TimedOut`] when nothing arrived within `timeout`; any
    /// other error aborts the read.
    fn read_some(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize>;
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

impl IpcMessage {
    /// Reads one message from `reader`, stopping at the first newline, at
    /// end of stream, or when more than [`MAX_MESSAGE_LEN`] bytes arrived. The
    /// *total* time is bounded by `deadline`, so a peer trickling one byte at
    /// a time cannot hold the connection either.
    ///
    /// # Errors
    ///
    /// [`IpcParseError::TimedOut`] when the deadline passes first,
    /// [`IpcParseError::TooLong`] past the size bound, otherwise as
    /// [`IpcMessage::decode`] (read errors count as `Unknown`).
    pub fn read_with_deadline(
        reader: &mut impl DeadlineRead,
        deadline: Duration,
    ) -> Result<Self, IpcParseError> {
        let start = Instant::now();
        let mut buf: Vec<u8> = Vec::with_capacity(MAX_MESSAGE_LEN + 1);
        let mut chunk = [0u8; MAX_MESSAGE_LEN + 1];
        loop {
            let remaining = deadline.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Err(IpcParseError::TimedOut);
            }
            // Never ask for more than the bound allows, plus one to detect overflow.
            let want = (MAX_MESSAGE_LEN + 1 - buf.len()).min(chunk.len());
            match reader.read_some(&mut chunk[..want], remaining) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n.min(want)]);
                    if buf.len() > MAX_MESSAGE_LEN {
                        return Err(IpcParseError::TooLong);
                    }
                    if buf.contains(&b'\n') {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    return Err(IpcParseError::TimedOut);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Err(IpcParseError::Unknown),
            }
        }
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

/// Pipe name for `user`: `\\.\pipe\TokenMon.<user>`. Characters outside
/// `[A-Za-z0-9_.-]` become `_`, so `a b` and `a_b` collide and any account can
/// pre-create the name: prefer [`session_pipe_name`].
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

/// Pipe name for one user in one logon session:
/// `\\.\pipe\TokenMon.<session id>.<user SID>`.
///
/// The SID (not the account name) is unique per account, so two users can
/// never map to the same name, and the session id keeps concurrent sessions
/// of one user (fast user switching, RDP) apart. Any character outside
/// `[A-Za-z0-9-]` is escaped as `_xxxx` (its code in hex), so the mapping is
/// injective. A pipe another account pre-created under this name is refused
/// by `FILE_FLAG_FIRST_PIPE_INSTANCE` and, because only this SID can open the
/// pipe, a squatter cannot impersonate the running instance either.
///
/// ```
/// use tokenmon_platform::shell::ipc::session_pipe_name;
/// assert_eq!(
///     session_pipe_name("S-1-5-21-1-2-3-1001", 2),
///     r"\\.\pipe\TokenMon.2.S-1-5-21-1-2-3-1001"
/// );
/// ```
#[must_use]
pub fn session_pipe_name(sid: &str, session_id: u32) -> String {
    use std::fmt::Write as _;
    let mut safe = String::with_capacity(sid.len());
    for c in sid.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            safe.push(c);
        } else {
            let _ = write!(safe, "_{:04x}", c as u32);
        }
    }
    format!(r"\\.\pipe\TokenMon.{session_id}.{safe}")
}
