use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

const SEALED_CONTEXT_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum ContextVaultError {
    #[error("context keychain unavailable: {0}")]
    Keychain(String),
    #[error("context encryption failed")]
    Encrypt,
    #[error("context decryption failed")]
    Decrypt,
    #[error("context envelope invalid: {0}")]
    Envelope(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealedContextEnvelope {
    pub version: u32,
    pub algorithm: String,
    pub nonce_base64: String,
    pub ciphertext_base64: String,
}

pub struct ContextSealer {
    key: Zeroizing<[u8; 32]>,
}

impl ContextSealer {
    pub fn from_macos_keychain(service: &str, account: &str) -> Result<Self, ContextVaultError> {
        #[cfg(target_os = "macos")]
        {
            let service = CString::new(service)
                .map_err(|_| ContextVaultError::Keychain("service contains NUL".to_owned()))?;
            let account = CString::new(account)
                .map_err(|_| ContextVaultError::Keychain("account contains NUL".to_owned()))?;
            let mut key = [0_u8; 32];
            let mut error: *mut c_char = std::ptr::null_mut();
            let status = unsafe {
                lumen_context_keychain_get_or_create(
                    service.as_ptr(),
                    account.as_ptr(),
                    key.as_mut_ptr(),
                    &mut error,
                )
            };
            if status != 0 {
                let message = if error.is_null() {
                    format!("OSStatus {status}")
                } else {
                    let message = unsafe { CStr::from_ptr(error) }
                        .to_string_lossy()
                        .into_owned();
                    unsafe { lumen_context_keychain_free(error) };
                    message
                };
                return Err(ContextVaultError::Keychain(message));
            }
            Ok(Self::from_key(key))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (service, account);
            Err(ContextVaultError::Keychain(
                "macOS Keychain is unavailable on this platform".to_owned(),
            ))
        }
    }

    #[doc(hidden)]
    pub fn from_key(key: [u8; 32]) -> Self {
        Self {
            key: Zeroizing::new(key),
        }
    }

    pub fn seal(
        &self,
        plaintext: &[u8],
        associated_data: &[u8],
    ) -> Result<SealedContextEnvelope, ContextVaultError> {
        let mut nonce = [0_u8; 12];
        getrandom::fill(&mut nonce).map_err(|_| ContextVaultError::Encrypt)?;
        let cipher = ChaCha20Poly1305::new(self.key.as_ref().into());
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: associated_data,
                },
            )
            .map_err(|_| ContextVaultError::Encrypt)?;
        Ok(SealedContextEnvelope {
            version: SEALED_CONTEXT_VERSION,
            algorithm: "chacha20_poly1305".to_owned(),
            nonce_base64: BASE64.encode(nonce),
            ciphertext_base64: BASE64.encode(ciphertext),
        })
    }

    pub fn open(
        &self,
        envelope: &SealedContextEnvelope,
        associated_data: &[u8],
    ) -> Result<Vec<u8>, ContextVaultError> {
        if envelope.version != SEALED_CONTEXT_VERSION || envelope.algorithm != "chacha20_poly1305" {
            return Err(ContextVaultError::Envelope(
                "unsupported version or algorithm".to_owned(),
            ));
        }
        let nonce = BASE64
            .decode(&envelope.nonce_base64)
            .map_err(|error| ContextVaultError::Envelope(error.to_string()))?;
        let nonce: [u8; 12] = nonce
            .try_into()
            .map_err(|_| ContextVaultError::Envelope("nonce length".to_owned()))?;
        let ciphertext = BASE64
            .decode(&envelope.ciphertext_base64)
            .map_err(|error| ContextVaultError::Envelope(error.to_string()))?;
        ChaCha20Poly1305::new(self.key.as_ref().into())
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: associated_data,
                },
            )
            .map_err(|_| ContextVaultError::Decrypt)
    }

    pub fn seal_json(
        &self,
        plaintext: &[u8],
        associated_data: &[u8],
    ) -> Result<Vec<u8>, ContextVaultError> {
        serde_json::to_vec(&self.seal(plaintext, associated_data)?)
            .map_err(|error| ContextVaultError::Envelope(error.to_string()))
    }
}

#[cfg(target_os = "macos")]
extern "C" {
    fn lumen_context_keychain_get_or_create(
        service_utf8: *const c_char,
        account_utf8: *const c_char,
        out_key: *mut u8,
        out_error: *mut *mut c_char,
    ) -> i32;
    fn lumen_context_keychain_free(value: *mut c_char);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_envelope_round_trips_and_rejects_wrong_aad() {
        let sealer = ContextSealer::from_key([7_u8; 32]);
        let envelope = sealer.seal(b"private context", b"capture-1").unwrap();
        assert_eq!(
            sealer.open(&envelope, b"capture-1").unwrap(),
            b"private context"
        );
        assert!(sealer.open(&envelope, b"capture-2").is_err());
    }
}
