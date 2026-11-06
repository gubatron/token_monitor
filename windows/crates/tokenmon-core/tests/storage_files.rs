//! `AppPaths`, atomic writes and `FileBackedStringStore` (spec 01 section 1.3.1-1.3.2).

#![allow(clippy::unwrap_used, clippy::assert_is_empty)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use tokenmon_core::storage::{
    AppPaths, FileBackedStringStore, NoopPermissions, PathsError, TextStore,
    home_directory_from_env, icacls_args, migrate_legacy_directory_if_needed, write_atomic,
};

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// The data directory is created as a directory.
#[test]
fn test_directory_is_created_as_directory() {
    let base = TempDir::new().unwrap();
    let dir = AppPaths::new(base.path()).directory().unwrap();
    assert!(dir.is_dir());
}

/// The default directory is `<base>/TokenMon`.
#[test]
fn test_default_uses_expected_subdirectory() {
    let base = TempDir::new().unwrap();
    let dir = AppPaths::new(base.path()).directory().unwrap();
    assert_eq!(dir.file_name().unwrap(), "TokenMon");
    assert_eq!(dir.parent().unwrap(), base.path());
}

/// A legacy `ModelMonitor` folder is moved when `TokenMon` is absent.
#[test]
fn test_moves_model_monitor_directory_when_current_is_absent() {
    let base = TempDir::new().unwrap();
    std::fs::create_dir(base.path().join("ModelMonitor")).unwrap();
    std::fs::write(base.path().join("ModelMonitor/auth_session.dat"), "s").unwrap();
    let dir = AppPaths::new(base.path()).directory().unwrap();
    assert!(dir.join("auth_session.dat").is_file());
    assert!(!base.path().join("ModelMonitor").exists());
}

/// An existing `TokenMon` folder is never merged or overwritten.
#[test]
fn test_does_not_overwrite_existing_current_directory() {
    let base = TempDir::new().unwrap();
    std::fs::create_dir(base.path().join("ModelMonitor")).unwrap();
    std::fs::create_dir(base.path().join("TokenMon")).unwrap();
    std::fs::write(base.path().join("ModelMonitor/old.dat"), "o").unwrap();
    let dir = AppPaths::new(base.path()).directory().unwrap();
    assert!(base.path().join("ModelMonitor/old.dat").is_file());
    assert!(!dir.join("old.dat").exists());
}

/// The generic migration helper reports whether it moved anything.
#[test]
fn test_migrate_helper_returns_whether_it_moved() {
    let base = TempDir::new().unwrap();
    let (a, b) = (base.path().join("a"), base.path().join("b"));
    assert!(!migrate_legacy_directory_if_needed(&a, &b).unwrap());
    std::fs::create_dir(&a).unwrap();
    assert!(migrate_legacy_directory_if_needed(&a, &b).unwrap());
    assert!(!migrate_legacy_directory_if_needed(&a, &b).unwrap());
}

fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
    move |name| {
        pairs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| OsString::from(*v))
    }
}

/// Production resolution prefers `LOCALAPPDATA`, then falls back.
#[test]
fn test_from_env_prefers_local_app_data() {
    let p = AppPaths::from_env(env_of(&[("LOCALAPPDATA", "/l"), ("HOME", "/h")])).unwrap();
    assert_eq!(p.base_directory(), Path::new("/l"));
    let p = AppPaths::from_env(env_of(&[("HOME", "/h")])).unwrap();
    assert_eq!(p.base_directory(), PathBuf::from("/h/.local/share"));
    let e = AppPaths::from_env(env_of(&[("LOCALAPPDATA", "")])).unwrap_err();
    assert_eq!(e, PathsError::NoBaseDirectory);
}

/// The real home directory comes from `USERPROFILE` or `HOME`.
#[test]
fn test_real_home_directory_is_the_user_home() {
    let home = home_directory_from_env(env_of(&[("USERPROFILE", "C:/u"), ("HOME", "/h")]));
    assert_eq!(home, Some(PathBuf::from("C:/u")));
    assert_eq!(home_directory_from_env(env_of(&[])), None);
}

/// Subdirectory names cannot escape the base.
#[test]
fn test_subdirectory_rejects_traversal() {
    let base = TempDir::new().unwrap();
    let paths = AppPaths::new(base.path());
    assert!(paths.subdirectory("../x").is_err());
    assert!(paths.subdirectory("a/b").is_err());
    assert!(paths.subdirectory("").is_err());
    assert!(paths.subdirectory("logs").unwrap().is_dir());
}

/// The ACL command removes inheritance and grants only the user.
#[test]
fn test_icacls_args_remove_inheritance_and_grant_owner() {
    let args = icacls_args(Path::new("C:/x"), "PC\\me");
    let texts: Vec<_> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        texts,
        ["C:/x", "/inheritance:r", "/grant:r", "PC\\me:(OI)(CI)F"]
    );
}

/// Unix directory and file modes are owner-only (0700 / 0600).
#[cfg(unix)]
#[test]
fn test_directory_and_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let base = TempDir::new().unwrap();
    let dir = AppPaths::new(base.path()).directory().unwrap();
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let store = FileBackedStringStore::new(&dir, "auth_");
    assert!(store.set("v", "k"));
    let mode = std::fs::metadata(dir.join("auth_k.dat"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

/// Set, get, overwrite and remove.
#[test]
fn test_file_backed_string_store_round_trip() {
    let dir = TempDir::new().unwrap();
    let store = FileBackedStringStore::new(dir.path(), "auth_").with_permissions(NoopPermissions);
    assert_eq!(store.value("k"), None);
    assert!(store.set("one", "k"));
    assert_eq!(store.value("k").as_deref(), Some("one"));
    assert!(store.set("two", "k"));
    assert_eq!(store.value("k").as_deref(), Some("two"));
    store.remove("k");
    assert_eq!(store.value("k"), None);
    store.remove("k");
}

/// The first write leaves exactly the target file, no staging files.
#[test]
fn test_file_store_first_write_is_atomic_and_leaves_no_staging_file() {
    let dir = TempDir::new().unwrap();
    let store =
        FileBackedStringStore::new(dir.path(), "activity_").with_permissions(NoopPermissions);
    assert!(store.set("v", "k"));
    assert_eq!(names(dir.path()), ["activity_k.dat"]);
}

/// A write into a missing directory reports failure and leaves nothing behind.
#[test]
fn test_file_store_reports_failed_write() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("nope");
    let store = FileBackedStringStore::new(&missing, "auth_").with_permissions(NoopPermissions);
    assert!(!store.set("v", "k"));
    assert!(!missing.exists());
}

/// Keys that could escape the directory are refused.
#[test]
fn test_file_store_rejects_unsafe_keys() {
    let dir = TempDir::new().unwrap();
    let store = FileBackedStringStore::new(dir.path(), "auth_").with_permissions(NoopPermissions);
    for key in ["", "../x", "a/b", "a\\b", "x:y", ".."] {
        assert!(!store.set("v", key), "{key:?}");
        assert_eq!(store.value(key), None);
    }
    assert!(names(dir.path()).is_empty());
}

/// `write_atomic` replaces an existing file and survives repeated writes.
#[test]
fn test_write_atomic_replaces_existing_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("f.dat");
    for i in 0..5 {
        write_atomic(&file, format!("v{i}").as_bytes(), &NoopPermissions).unwrap();
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v4");
    assert_eq!(names(dir.path()), ["f.dat"]);
}
