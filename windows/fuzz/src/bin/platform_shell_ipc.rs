//! Invariants: decoding never panics; a decoded message re-encodes to bytes
//! that decode to the same message; the bounded reader agrees with `decode`
//! for inputs within the bound and rejects anything longer.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_platform::shell::IpcMessage;
use tokenmon_platform::shell::ipc::MAX_MESSAGE_LEN;

fuzz_target!(|data: &[u8]| {
    let decoded = IpcMessage::decode(data);
    if let Ok(msg) = decoded {
        assert_eq!(IpcMessage::decode(&msg.encode()), Ok(msg));
    }
    if data.len() <= MAX_MESSAGE_LEN {
        assert_eq!(IpcMessage::read_from(data), decoded);
    } else {
        assert!(IpcMessage::read_from(data).is_err());
    }
});
