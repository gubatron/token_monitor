//! #47: `PlatformPermissions` delegates to the implementation the platform
//! layer registers (the SID-based DACL code lives there; this crate forbids
//! `unsafe`). Own test binary because the registration is process-global.
#![allow(clippy::unwrap_used)]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tempfile::TempDir;
use tokenmon_core::storage::{
    AppPaths, OwnerOnlyPermissions, PlatformPermissions, icacls_program,
    register_platform_permissions,
};

#[derive(Debug, Default)]
struct Recording(Mutex<Vec<PathBuf>>);

#[derive(Debug)]
struct Handle(Arc<Recording>);

impl OwnerOnlyPermissions for Handle {
    fn restrict_directory(&self, path: &Path) -> io::Result<()> {
        self.0.0.lock().unwrap().push(path.to_owned());
        Ok(())
    }
    fn restrict_file(&self, path: &Path) -> io::Result<()> {
        self.0.0.lock().unwrap().push(path.to_owned());
        Ok(())
    }
}

#[test]
fn registered_implementation_receives_directory_restrictions() {
    let recording = Arc::new(Recording::default());
    assert!(register_platform_permissions(Arc::new(Handle(
        recording.clone()
    ))));
    assert!(
        !register_platform_permissions(Arc::new(Handle(recording.clone()))),
        "a second registration is refused"
    );

    let base = TempDir::new().unwrap();
    let paths = AppPaths::new(base.path());
    let dir = paths.directory().unwrap();
    assert!(recording.0.lock().unwrap().contains(&dir));
    PlatformPermissions.restrict_file(&dir.join("x")).unwrap();
    assert!(recording.0.lock().unwrap().contains(&dir.join("x")));
}

#[test]
fn icacls_is_addressed_by_absolute_system_path() {
    assert_eq!(
        icacls_program(Some(r"D:\Win\")),
        r"D:\Win\System32\icacls.exe"
    );
    for hostile in [None, Some("icacls"), Some(r"..\evil"), Some(""), Some("C:")] {
        assert_eq!(icacls_program(hostile), r"C:\Windows\System32\icacls.exe");
    }
}
