//! Protected secret storage.
//!
//! Things that belong here and **not** in a SQLite row: StageLink's own
//! long-term signing key, per-receiver shared secrets and anything else whose
//! disclosure would let someone join a session or impersonate the desktop.
//!
//! On Windows the implementation is DPAPI (`CryptProtectData`), which encrypts
//! with a key derived from the logged-in user's credentials. The ciphertext is
//! written next to the database with user-only semantics: copying the file to
//! another machine or another user account renders it undecryptable. That is
//! the right property for a venue laptop — a stolen disk image does not hand
//! over the ability to join the monitor system.
//!
//! What this is *not*: a defence against malware already running as the same
//! user. Nothing a desktop application can do protects against that, and
//! pretending otherwise would be dishonest.

use std::path::{Path, PathBuf};

use crate::error::{PersistenceError, Result};

/// A named secret store backed by the OS.
pub struct SecretStore {
    directory: PathBuf,
}

impl SecretStore {
    /// Creates a store writing into `directory`.
    pub fn new(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        std::fs::create_dir_all(&directory).map_err(|source| PersistenceError::DataDirectory {
            path: directory.display().to_string(),
            source,
        })?;
        Ok(Self { directory })
    }

    fn path_for(&self, name: &str) -> Result<PathBuf> {
        // Secret names come from our own code, never from the network, but a
        // path traversal here would be unrecoverable so it is rejected outright.
        if name.is_empty()
            || name.contains("..")
            || name.contains('/')
            || name.contains('\\')
            || name.contains(':')
        {
            return Err(PersistenceError::SecretStore(format!(
                "invalid secret name {name:?}"
            )));
        }
        Ok(self.directory.join(format!("{name}.dpapi")))
    }

    /// Stores `secret` under `name`, replacing any previous value.
    pub fn set(&self, name: &str, secret: &[u8]) -> Result<()> {
        let path = self.path_for(name)?;
        let protected = platform::protect(secret, name)?;
        // Write-then-rename so a crash mid-write cannot leave a truncated
        // secret that would lock every receiver out.
        let temporary = path.with_extension("dpapi.tmp");
        std::fs::write(&temporary, &protected).map_err(|source| {
            PersistenceError::DataDirectory { path: temporary.display().to_string(), source }
        })?;
        std::fs::rename(&temporary, &path).map_err(|source| PersistenceError::DataDirectory {
            path: path.display().to_string(),
            source,
        })?;
        Ok(())
    }

    /// Reads the secret stored under `name`, or `None` if there is none.
    pub fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path_for(name)?;
        let protected = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(PersistenceError::DataDirectory {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        platform::unprotect(&protected, name).map(Some)
    }

    /// Removes a secret. Succeeds whether or not it existed.
    pub fn delete(&self, name: &str) -> Result<()> {
        let path = self.path_for(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(PersistenceError::DataDirectory {
                path: path.display().to_string(),
                source,
            }),
        }
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

#[cfg(windows)]
mod platform {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    use crate::error::{PersistenceError, Result};

    /// Encrypts `plaintext` to the current user.
    ///
    /// `name` is bound in as optional entropy, so a secret written under one
    /// name cannot be decrypted by asking for another — it stops a mix-up (or a
    /// file swap) from silently succeeding with the wrong key material.
    pub fn protect(plaintext: &[u8], name: &str) -> Result<Vec<u8>> {
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(plaintext.len())
                .map_err(|_| PersistenceError::SecretStore("secret is too large".into()))?,
            pbData: plaintext.as_ptr().cast_mut(),
        };
        let mut entropy_bytes = name.as_bytes().to_vec();
        let mut entropy = CRYPT_INTEGER_BLOB {
            cbData: entropy_bytes.len() as u32,
            pbData: entropy_bytes.as_mut_ptr(),
        };
        let mut output = CRYPT_INTEGER_BLOB::default();

        // SAFETY: both input blobs point at live buffers for the duration of
        // the call; `output` is freed with LocalFree below.
        unsafe {
            CryptProtectData(
                &raw mut input,
                windows::core::PCWSTR::null(),
                Some(&raw const entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        }
        .map_err(|error| {
            PersistenceError::SecretStore(format!("CryptProtectData failed: {error}"))
        })?;

        let protected = copy_and_free(&mut output);
        // Keep `entropy` alive until after the call.
        drop(entropy_bytes);
        let _ = &mut entropy;
        Ok(protected)
    }

    /// Decrypts data produced by [`protect`] under the same `name`.
    pub fn unprotect(ciphertext: &[u8], name: &str) -> Result<Vec<u8>> {
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(ciphertext.len())
                .map_err(|_| PersistenceError::SecretStore("secret is too large".into()))?,
            pbData: ciphertext.as_ptr().cast_mut(),
        };
        let mut entropy_bytes = name.as_bytes().to_vec();
        let entropy = CRYPT_INTEGER_BLOB {
            cbData: entropy_bytes.len() as u32,
            pbData: entropy_bytes.as_mut_ptr(),
        };
        let mut output = CRYPT_INTEGER_BLOB::default();

        // SAFETY: as in `protect`.
        unsafe {
            CryptUnprotectData(
                &raw mut input,
                None,
                Some(&raw const entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        }
        .map_err(|error| {
            PersistenceError::SecretStore(format!(
                "CryptUnprotectData failed (wrong user, or the file was tampered with): {error}"
            ))
        })?;

        let plaintext = copy_and_free(&mut output);
        drop(entropy_bytes);
        Ok(plaintext)
    }

    /// Copies a DPAPI output blob into a `Vec` and releases the OS allocation.
    fn copy_and_free(blob: &mut CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if blob.pbData.is_null() || blob.cbData == 0 {
            return Vec::new();
        }
        // SAFETY: DPAPI guarantees `cbData` readable bytes at `pbData`.
        let bytes =
            unsafe { core::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
        // SAFETY: DPAPI allocated this with LocalAlloc.
        unsafe { LocalFree(Some(HLOCAL(blob.pbData.cast()))) };
        blob.pbData = core::ptr::null_mut();
        blob.cbData = 0;
        bytes
    }
}

#[cfg(not(windows))]
mod platform {
    use crate::error::{PersistenceError, Result};

    /// No OS-protected store is wired up for non-Windows builds.
    ///
    /// Deliberately an error rather than a plaintext fallback: writing key
    /// material unencrypted "just for now" is exactly the kind of insecure
    /// default this project forbids.
    pub fn protect(_plaintext: &[u8], _name: &str) -> Result<Vec<u8>> {
        Err(PersistenceError::SecretStore(
            "no OS-protected secret store on this platform; StageLink Core targets Windows".into(),
        ))
    }

    pub fn unprotect(_ciphertext: &[u8], _name: &str) -> Result<Vec<u8>> {
        Err(PersistenceError::SecretStore(
            "no OS-protected secret store on this platform; StageLink Core targets Windows".into(),
        ))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SecretStore) {
        let temp = tempfile::tempdir().unwrap();
        let store = SecretStore::new(temp.path().join("secrets")).unwrap();
        (temp, store)
    }

    #[test]
    fn a_secret_round_trips() {
        let (_temp, store) = store();
        let secret = b"a-32-byte-session-key-goes-here!";
        store.set("session-key", secret).unwrap();
        assert_eq!(store.get("session-key").unwrap().as_deref(), Some(&secret[..]));
    }

    #[test]
    fn a_missing_secret_reads_as_none() {
        let (_temp, store) = store();
        assert_eq!(store.get("never-written").unwrap(), None);
    }

    #[test]
    fn the_stored_file_does_not_contain_the_plaintext() {
        let (_temp, store) = store();
        let secret = b"SUPER-SECRET-SIGNING-KEY";
        store.set("signing-key", secret).unwrap();

        let raw = std::fs::read(store.directory().join("signing-key.dpapi")).unwrap();
        assert!(
            !raw.windows(secret.len()).any(|window| window == secret),
            "the secret must not be recoverable from the file by inspection"
        );
    }

    #[test]
    fn a_secret_cannot_be_read_back_under_a_different_name() {
        let (_temp, store) = store();
        store.set("key-a", b"value-a").unwrap();

        // Swap the file into another name, as a confused-deputy attempt would.
        let from = store.directory().join("key-a.dpapi");
        let to = store.directory().join("key-b.dpapi");
        std::fs::copy(&from, &to).unwrap();

        assert!(
            store.get("key-b").is_err(),
            "name-bound entropy must stop a swapped secret file from decrypting"
        );
    }

    #[test]
    fn tampering_with_the_ciphertext_is_detected() {
        let (_temp, store) = store();
        store.set("key", b"value").unwrap();

        let path = store.directory().join("key.dpapi");
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();

        assert!(store.get("key").is_err(), "DPAPI must reject tampered data");
    }

    #[test]
    fn overwriting_replaces_the_value() {
        let (_temp, store) = store();
        store.set("key", b"first").unwrap();
        store.set("key", b"second").unwrap();
        assert_eq!(store.get("key").unwrap().as_deref(), Some(&b"second"[..]));
    }

    #[test]
    fn deleting_is_idempotent() {
        let (_temp, store) = store();
        store.set("key", b"value").unwrap();
        store.delete("key").unwrap();
        store.delete("key").unwrap();
        assert_eq!(store.get("key").unwrap(), None);
    }

    #[test]
    fn path_traversal_in_a_secret_name_is_refused() {
        let (_temp, store) = store();
        for name in ["", "../escape", "sub/dir", "sub\\dir", "C:evil"] {
            assert!(
                store.set(name, b"x").is_err(),
                "{name:?} must be refused as a secret name"
            );
        }
    }

    #[test]
    fn an_empty_secret_round_trips() {
        let (_temp, store) = store();
        store.set("empty", b"").unwrap();
        assert_eq!(store.get("empty").unwrap(), Some(Vec::new()));
    }
}
