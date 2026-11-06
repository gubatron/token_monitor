//! Post-download verification hook.
//!
//! The SHA-256 check against GitHub's asset digest is done by the installer
//! itself. This module is where publisher verification plugs in.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use tokenmon_core::update::InstallError;

/// Verifies a file about to be installed.
pub trait SignatureVerifier: Send + Sync {
    /// Checks the executable (`.exe`) that will replace the running one.
    fn verify_exe(&self, path: &Path) -> Result<(), InstallError>;
    /// Checks the Windows Installer package that will be run.
    fn verify_msi(&self, path: &Path) -> Result<(), InstallError>;
}

/// Structural checks only: the file really is a PE image / an MSI (OLE)
/// container. This is NOT publisher verification.
///
/// TODO(signing): once releases are Authenticode-signed, add a verifier that
/// calls `WinVerifyTrust` (`WINTRUST_ACTION_GENERIC_VERIFY_V2`) and pins the
/// signer subject/thumbprint, and fail closed. Until then unsigned builds are
/// accepted because the SHA-256 digest comes from the same GitHub release.
#[derive(Debug, Clone, Copy, Default)]
pub struct PeHeaderVerifier;

fn bad(msg: &str) -> InstallError {
    InstallError::Verification(msg.to_owned())
}

/// OLE compound-file magic that every `.msi` starts with.
const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

fn is_pe(path: &Path) -> std::io::Result<bool> {
    let mut f = File::open(path)?;
    let mut dos = [0u8; 64];
    if f.read_exact(&mut dos).is_err() || &dos[..2] != b"MZ" {
        return Ok(false);
    }
    let offset = u32::from_le_bytes([dos[0x3C], dos[0x3D], dos[0x3E], dos[0x3F]]);
    f.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut sig = [0u8; 4];
    Ok(f.read_exact(&mut sig).is_ok() && &sig == b"PE\0\0")
}

impl SignatureVerifier for PeHeaderVerifier {
    fn verify_exe(&self, path: &Path) -> Result<(), InstallError> {
        match is_pe(path) {
            Ok(true) => Ok(()),
            _ => Err(bad("The download is not a Windows program.")),
        }
    }

    fn verify_msi(&self, path: &Path) -> Result<(), InstallError> {
        let head = super::archive::read_prefix(path, 8)
            .map_err(|_| bad("The installer could not be read."))?;
        if head == OLE_MAGIC {
            Ok(())
        } else {
            Err(bad("The download is not a Windows installer."))
        }
    }
}
