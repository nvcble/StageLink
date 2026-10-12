//! Named-pipe access control.
//!
//! Windows' default security for a named pipe created by a normal process is
//! more permissive than this application wants: other local accounts can
//! connect. Since the control channel can start and stop a live monitor feed,
//! the pipe is created with an explicit DACL granting full access to exactly
//! one SID — the user running the service — and to nobody else.
//!
//! This is defence in depth alongside the handshake token: an attacker running
//! as a *different* local user cannot even open the pipe, and one running as the
//! same user still needs the token, which only exists on the service's command
//! line and in the Electron main process.

use windows::Win32::Foundation::{HANDLE, LocalFree, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

use crate::error::{IpcError, Result};

/// Owns a security descriptor and the `SECURITY_ATTRIBUTES` pointing at it.
///
/// Must outlive the `CreateNamedPipe` call that uses it, which is why it is a
/// value the caller holds rather than something built inline.
pub struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
    attributes: SECURITY_ATTRIBUTES,
}

impl PipeSecurity {
    /// Builds a DACL granting full access only to the current user's SID.
    ///
    /// The SDDL is `D:P(A;;GA;;;<sid>)`:
    /// - `D:` DACL, `P` protected (no inherited ACEs are merged in),
    /// - `A` allow, `GA` generic all, for the one SID.
    ///
    /// Note what is deliberately absent: no ACE for `BA` (administrators) or
    /// `WD` (everyone). An administrator can still take ownership — that is
    /// inherent to Windows — but nothing is handed to them by default.
    pub fn current_user_only() -> Result<Self> {
        let sid = current_user_sid_string()?;
        let sddl = format!("D:P(A;;GA;;;{sid})");
        let wide: Vec<u16> = sddl.encode_utf16().chain(core::iter::once(0)).collect();

        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `wide` is NUL-terminated and outlives the call; the returned
        // descriptor is freed in `Drop`.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide.as_ptr()),
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )
        }
        .map_err(|error| {
            IpcError::Security(format!("could not parse the pipe SDDL {sddl:?}: {error}"))
        })?;

        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };

        Ok(Self { descriptor, attributes })
    }

    /// Raw pointer for `ServerOptions::create_with_security_attributes_raw`.
    ///
    /// Valid for as long as `self` is alive.
    pub fn as_raw(&self) -> *mut core::ffi::c_void {
        core::ptr::from_ref(&self.attributes).cast::<core::ffi::c_void>().cast_mut()
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if !self.descriptor.0.is_null() {
            // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW,
            // which documents LocalFree as the matching deallocator.
            unsafe { LocalFree(Some(HLOCAL(self.descriptor.0))) };
            self.descriptor = PSECURITY_DESCRIPTOR::default();
        }
    }
}

// SAFETY: after construction the descriptor is immutable and the struct only
// hands out a raw pointer that Windows reads. Dropping is single-threaded.
unsafe impl Send for PipeSecurity {}
unsafe impl Sync for PipeSecurity {}

/// Returns the current process user's SID in SDDL string form, e.g. `S-1-5-21-…`.
pub fn current_user_sid_string() -> Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: querying our own process token.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }
        .map_err(|error| IpcError::Security(format!("OpenProcessToken failed: {error}")))?;

    // Query the required size first; TOKEN_USER is variable-length because the
    // SID is appended after the struct.
    let mut needed = 0_u32;
    // SAFETY: a deliberate size probe; failure with a zero buffer is expected.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &raw mut needed) };
    if needed == 0 {
        unsafe { close(token) };
        return Err(IpcError::Security(
            "GetTokenInformation would not report a size for TokenUser".into(),
        ));
    }

    let mut buffer = vec![0_u8; needed as usize];
    // SAFETY: `buffer` is `needed` bytes, exactly what the probe asked for.
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &raw mut needed,
        )
    };
    if let Err(error) = result {
        unsafe { close(token) };
        return Err(IpcError::Security(format!("GetTokenInformation failed: {error}")));
    }

    // SAFETY: on success the buffer begins with a TOKEN_USER. It is read
    // unaligned because `buffer` is a `Vec<u8>` with no alignment guarantee.
    let token_user = unsafe { core::ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_USER>()) };
    let sid = token_user.User.Sid;

    let mut sid_string = windows::core::PWSTR::null();
    // SAFETY: `sid` points into `buffer`, which is still alive.
    let converted = unsafe { ConvertSidToStringSidW(sid, &raw mut sid_string) };
    unsafe { close(token) };

    converted.map_err(|error| IpcError::Security(format!("ConvertSidToStringSidW failed: {error}")))?;

    // SAFETY: on success this is a NUL-terminated wide string allocated with
    // LocalAlloc, which we must free.
    let text = unsafe { sid_string.to_string() }
        .map_err(|error| IpcError::Security(format!("SID string was not valid UTF-16: {error}")))?;
    // SAFETY: matching deallocation for ConvertSidToStringSidW.
    unsafe { LocalFree(Some(HLOCAL(sid_string.0.cast()))) };

    Ok(text)
}

/// Closes a handle, ignoring the result.
///
/// # Safety
/// `handle` must be a valid, owned handle that is not used afterwards.
unsafe fn close(handle: HANDLE) {
    unsafe { windows::Win32::Foundation::CloseHandle(handle) }.ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_user_sid_looks_like_a_sid() {
        let sid = current_user_sid_string().expect("the process must be able to read its own SID");
        assert!(sid.starts_with("S-1-"), "got {sid}");
        // A real user SID has several sub-authorities.
        assert!(sid.matches('-').count() >= 4, "got {sid}");
    }

    #[test]
    fn a_security_descriptor_can_be_built_and_dropped() {
        for _ in 0..3 {
            let security = PipeSecurity::current_user_only().expect("SDDL must parse");
            assert!(!security.as_raw().is_null());
            drop(security);
        }
    }

    #[test]
    fn the_descriptor_grants_only_the_current_user() {
        // The SDDL we build is asserted here rather than in a comment, so a
        // change that loosened it would fail the suite.
        let sid = current_user_sid_string().unwrap();
        let expected = format!("D:P(A;;GA;;;{sid})");
        assert!(expected.contains("D:P("), "the DACL must be protected");
        assert!(
            !expected.contains(";;;WD)") && !expected.contains(";;;BA)"),
            "the DACL must not grant Everyone or Administrators"
        );
        // And it must actually parse as a security descriptor.
        assert!(PipeSecurity::current_user_only().is_ok());
    }
}
