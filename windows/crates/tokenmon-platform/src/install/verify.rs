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

/// Whether a `Zone.Identifier` stream's text marks the file as downloaded from
/// the Internet or a restricted zone (`ZoneId` 3 or 4).
///
/// # Examples
///
/// ```
/// use tokenmon_platform::install::verify::zone_id_marks_internet;
///
/// assert!(zone_id_marks_internet("[ZoneTransfer]\r\nZoneId=3\r\n"));
/// assert!(!zone_id_marks_internet("[ZoneTransfer]\r\nZoneId=2\r\n"));
/// assert!(!zone_id_marks_internet(""));
/// ```
pub fn zone_id_marks_internet(contents: &str) -> bool {
    contents
        .lines()
        .filter_map(|l| l.trim().strip_prefix("ZoneId="))
        .any(|v| v.trim().parse::<u32>().is_ok_and(|z| z >= 3))
}

/// The text of the file's `Zone.Identifier` alternate data stream, if any.
/// Only Windows has such streams.
fn read_zone_identifier(path: &Path) -> Option<String> {
    if cfg!(windows) {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        std::fs::read_to_string(stream).ok()
    } else {
        None
    }
}

impl SignatureVerifier for PeHeaderVerifier {
    fn verify_exe(&self, path: &Path) -> Result<(), InstallError> {
        if !matches!(is_pe(path), Ok(true)) {
            return Err(bad("The download is not a Windows program."));
        }
        if read_zone_identifier(path).is_some_and(|t| zone_id_marks_internet(&t)) {
            return Err(bad(
                "Windows would block this unsigned download from opening.",
            ));
        }
        Ok(())
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

/// How strictly downloaded builds are checked for a publisher signature.
///
/// The application chooses this in exactly one place (see
/// [`verifier_for_policy`]); switching policy is a one-line change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignaturePolicy {
    /// Accept an unsigned build when its SHA-256 matches the digest GitHub
    /// publishes for the same release and the file is structurally a PE/MSI.
    /// This is the default until releases are Authenticode-signed.
    #[default]
    AcceptUnsignedWithDigest,
    /// Additionally require a valid Authenticode signature (`WinVerifyTrust`).
    /// Unsigned builds are refused. Fails closed on non-Windows hosts.
    RequireAuthenticode,
}

/// Builds the verifier that enforces `policy`.
///
/// # Examples
///
/// ```
/// use tokenmon_platform::install::verify::{SignaturePolicy, verifier_for_policy};
///
/// let lenient = verifier_for_policy(SignaturePolicy::AcceptUnsignedWithDigest);
/// // Not a PE file: rejected under every policy.
/// assert!(lenient.verify_exe(std::path::Path::new("missing.exe")).is_err());
/// ```
pub fn verifier_for_policy(policy: SignaturePolicy) -> std::sync::Arc<dyn SignatureVerifier> {
    match policy {
        SignaturePolicy::AcceptUnsignedWithDigest => std::sync::Arc::new(PeHeaderVerifier),
        SignaturePolicy::RequireAuthenticode => std::sync::Arc::new(AuthenticodeVerifier),
    }
}

/// Structural checks plus a mandatory Authenticode signature check.
///
/// On Windows the trust check is `WinVerifyTrust` with
/// `WINTRUST_ACTION_GENERIC_VERIFY_V2` (revocation checks on the whole chain,
/// no UI). Elsewhere there is no verifier, so every file is refused.
#[derive(Debug, Clone, Copy, Default)]
pub struct AuthenticodeVerifier;

impl SignatureVerifier for AuthenticodeVerifier {
    fn verify_exe(&self, path: &Path) -> Result<(), InstallError> {
        PeHeaderVerifier.verify_exe(path)?;
        authenticode::verify_file(path)
    }

    fn verify_msi(&self, path: &Path) -> Result<(), InstallError> {
        PeHeaderVerifier.verify_msi(path)?;
        authenticode::verify_file(path)
    }
}

#[cfg(windows)]
mod authenticode {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use tokenmon_core::update::InstallError;
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CHOICE_FILE, WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE, WinVerifyTrust,
    };

    /// Verifies the Authenticode signature of `path`; any non-zero
    /// `WinVerifyTrust` result (including "not signed") is a refusal.
    pub fn verify_file(path: &Path) -> Result<(), InstallError> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut file = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: wide.as_ptr(),
            hFile: std::ptr::null_mut(),
            pgKnownSubject: std::ptr::null_mut(),
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            pPolicyCallbackData: std::ptr::null_mut(),
            pSIPClientData: std::ptr::null_mut(),
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 {
                pFile: &raw mut file,
            },
            dwStateAction: WTD_STATEACTION_VERIFY,
            hWVTStateData: std::ptr::null_mut(),
            pwszURLReference: std::ptr::null_mut(),
            dwProvFlags: 0,
            dwUIContext: 0,
            pSignatureSettings: std::ptr::null_mut(),
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        // SAFETY: `data`, `file` and `wide` outlive both calls; the structs
        // are fully initialised; the CLOSE call releases the state handle.
        let status = unsafe {
            let status = WinVerifyTrust(
                std::ptr::null_mut(),
                &raw mut action,
                (&raw mut data).cast(),
            );
            data.dwStateAction = WTD_STATEACTION_CLOSE;
            WinVerifyTrust(
                std::ptr::null_mut(),
                &raw mut action,
                (&raw mut data).cast(),
            );
            status
        };
        if status == 0 {
            Ok(())
        } else {
            Err(InstallError::Verification(
                "The download is not signed by a trusted publisher.".to_owned(),
            ))
        }
    }
}

#[cfg(not(windows))]
mod authenticode {
    use std::path::Path;

    use tokenmon_core::update::InstallError;

    /// No Authenticode on this host: fail closed.
    pub fn verify_file(_path: &Path) -> Result<(), InstallError> {
        Err(InstallError::Verification(
            "Signature verification is only available on Windows.".to_owned(),
        ))
    }
}
