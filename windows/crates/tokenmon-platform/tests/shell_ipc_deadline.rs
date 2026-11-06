//! Single-instance IPC hardening (issue #45): a stalled or trickling client
//! cannot hold the server, and pipe names are unique per SID and session.

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use tokenmon_platform::shell::ipc::{DeadlineRead, MAX_MESSAGE_LEN, session_pipe_name};
use tokenmon_platform::shell::{IpcMessage, IpcParseError};

/// A peer that connected and never sends anything: every read waits out its
/// whole timeout, exactly like an overlapped read on a silent pipe.
struct Stalled;

impl DeadlineRead for Stalled {
    fn read_some(&mut self, _buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        std::thread::sleep(timeout.min(Duration::from_millis(50)));
        Err(io::ErrorKind::TimedOut.into())
    }
}

/// A peer that sends one byte per read forever (slowloris).
struct Trickle;

impl DeadlineRead for Trickle {
    fn read_some(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        std::thread::sleep(timeout.min(Duration::from_millis(20)));
        buf[0] = b'T';
        Ok(1)
    }
}

/// Scripted chunks, then end of stream.
struct Scripted(VecDeque<Vec<u8>>);

impl DeadlineRead for Scripted {
    fn read_some(&mut self, buf: &mut [u8], _timeout: Duration) -> io::Result<usize> {
        match self.0.pop_front() {
            None => Ok(0),
            Some(chunk) => {
                let n = chunk.len().min(buf.len());
                buf[..n].copy_from_slice(&chunk[..n]);
                Ok(n)
            }
        }
    }
}

#[test]
fn a_stalled_client_times_out_instead_of_blocking_the_server() {
    let start = Instant::now();
    let r = IpcMessage::read_with_deadline(&mut Stalled, Duration::from_millis(200));
    assert_eq!(r, Err(IpcParseError::TimedOut));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_trickling_client_is_bounded_by_the_total_deadline() {
    let start = Instant::now();
    let r = IpcMessage::read_with_deadline(&mut Trickle, Duration::from_millis(150));
    // Either the total deadline or the size bound ends it, never a hang.
    assert!(matches!(
        r,
        Err(IpcParseError::TimedOut | IpcParseError::TooLong)
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_complete_message_is_delivered_even_when_split_across_reads() {
    let mut r = Scripted(VecDeque::from([b"TM1 ".to_vec(), b"show\n".to_vec()]));
    assert_eq!(
        IpcMessage::read_with_deadline(&mut r, Duration::from_secs(1)),
        Ok(IpcMessage::Show)
    );
    // No trailing newline: the close of the stream ends the message.
    let mut r = Scripted(VecDeque::from([b"TM1 show".to_vec()]));
    assert_eq!(
        IpcMessage::read_with_deadline(&mut r, Duration::from_secs(1)),
        Ok(IpcMessage::Show)
    );
}

#[test]
fn oversized_and_unknown_messages_are_rejected() {
    let mut r = Scripted(VecDeque::from([vec![b'x'; MAX_MESSAGE_LEN + 1]]));
    assert_eq!(
        IpcMessage::read_with_deadline(&mut r, Duration::from_secs(1)),
        Err(IpcParseError::TooLong)
    );
    let mut r = Scripted(VecDeque::from([b"nope\n".to_vec()]));
    assert_eq!(
        IpcMessage::read_with_deadline(&mut r, Duration::from_secs(1)),
        Err(IpcParseError::Unknown)
    );
}

#[test]
fn session_pipe_names_are_unique_per_sid_and_session() {
    let a = session_pipe_name("S-1-5-21-1-2-3-1001", 1);
    assert_eq!(a, r"\\.\pipe\TokenMon.1.S-1-5-21-1-2-3-1001");
    assert_ne!(a, session_pipe_name("S-1-5-21-1-2-3-1002", 1));
    assert_ne!(a, session_pipe_name("S-1-5-21-1-2-3-1001", 2));
    // Unusual characters are escaped injectively (the old `a b` / `a_b`
    // collision cannot happen), and never produce a path separator.
    let b = session_pipe_name("a b", 1);
    let c = session_pipe_name("a_b", 1);
    assert_ne!(b, c);
    assert!(!session_pipe_name(r"a\b/c", 1)["\\\\.\\pipe\\".len()..].contains(['\\', '/']));
}

/// Real pipe: a client that connects and stalls must not stop the next client,
/// and dropping the server must not hang. Runs on the Windows CI.
#[cfg(windows)]
#[test]
fn windows_pipe_survives_a_stalled_client_and_drops_promptly() {
    use std::fs::OpenOptions;
    use std::sync::mpsc;
    use tokenmon_platform::shell::ipc::IpcClient;
    use tokenmon_platform::shell::windows::{PipeClient, PipeServer, current_user_pipe_name};

    let name = format!(
        "{}.test{}",
        current_user_pipe_name().unwrap(),
        std::process::id()
    );
    let (tx, rx) = mpsc::channel();
    let server = PipeServer::start(&name, move |m| {
        let _ = tx.send(m);
    })
    .unwrap();

    // Squatting: a second server on the same name is refused.
    assert!(PipeServer::start(&name, |_| {}).is_err());

    // The stalled client: connected, writes nothing, never closes.
    let stalled = OpenOptions::new().write(true).open(&name).unwrap();

    // A well-behaved client still gets through once the deadline frees the pipe.
    PipeClient::new(name.clone())
        .send(&IpcMessage::Show)
        .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)),
        Ok(IpcMessage::Show)
    );

    // A second stalled client, then quit: Drop must not wait for it.
    let stalled2 = OpenOptions::new().write(true).open(&name);
    let start = Instant::now();
    drop(server);
    assert!(start.elapsed() < Duration::from_secs(5));
    drop((stalled, stalled2));
}
