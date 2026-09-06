//! End-to-end encryption for synced notes.
//!
//! Every object the sync engine puts on the backend — markdown bodies and
//! sidecars alike — is sealed with **XChaCha20-Poly1305** under a key
//! derived from the user's encryption password via **Argon2id**. The
//! backend only ever stores ciphertext; a device that knows the password
//! can decrypt it, and one that doesn't cannot.
//!
//! ## Key sharing across devices
//!
//! The Argon2id **salt is not secret**: each encrypted blob embeds the salt
//! in its header, and the [`Verifier`] object stored on the backend
//! carries it too. Every device derives the same key from the same
//! password + salt, so data encrypted on device A decrypts on device B
//! (and vice versa) as long as both entered the same password. If the
//! verifier object is ever deleted, the salt can still be recovered from
//! any object header, so no data is lost by removing it.
//!
//! ## Blob format
//!
//! ```text
//! "NOTASENC" | version (1) | salt (16) | nonce (24) | ciphertext + tag
//! ```
//!
//! XChaCha20-Poly1305 authenticates the ciphertext and appends a 16-byte
//! tag, so a wrong key, a tampered blob, or a foreign plaintext object all
//! fail decryption loudly instead of producing garbage.
//!
//! # Examples
//!
//! ```
//! use notas_core::crypto::Cipher;
//!
//! let cipher = Cipher::derive("correct horse battery staple", [0; 16]).unwrap();
//! let blob = cipher.encrypt(b"secret").unwrap();
//! assert_eq!(cipher.decrypt(&blob).unwrap(), b"secret");
//! ```

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

/// Minimum length enforced for the encryption password when it is enabled
/// (a short password silently undermines the encryption).
pub const MIN_PASSWORD_LEN: usize = 8;

/// Magic bytes prefixing every encrypted blob.
const MAGIC: &[u8] = b"NOTASENC";

/// Blob format version; bump on any layout change and reject older
/// versions explicitly instead of misreading them.
const VERSION: u8 = 1;

/// Argon2id salt length (16 bytes is the OWASP minimum). Public so
/// callers can size salt buffers without duplicating the constant.
pub const SALT_LEN: usize = 16;

/// XChaCha20-Poly1305 nonce length (192 bits, random-nonce safe).
const NONCE_LEN: usize = 24;

/// Fixed header size: magic + version + salt + nonce.
const HEADER_LEN: usize = MAGIC.len() + 1 + SALT_LEN + NONCE_LEN;

/// Known plaintext sealed inside the [`Verifier`]; decrypting it and
/// comparing it proves that the entered password derives the key that
/// encrypted the backend's notes.
const VERIFIER_PLAINTEXT: &[u8] = b"notas sync encryption verifier";

/// Argon2id cost parameters. Memory 19 MiB / iterations 2 / lanes 1 is the
/// OWASP-recommended baseline; the key is derived **once per sync run**
/// (never per note), so even on a slow machine the cost is a fraction of a
/// second.
const ARGON2_M_COST: u32 = 19 * 1024; // KiB
const ARGON2_T_COST: u32 = 2;
const ARGON2_P_COST: u32 = 1;

/// The per-backend encryption key plus the salt it was derived from.
///
/// One instance is created per sync run (see the sync engine) and used for
/// every object of that run, so Argon2id runs exactly once per sync.
#[derive(Debug, Clone)]
pub struct Cipher {
    key: [u8; 32],
    salt: [u8; SALT_LEN],
}

/// Plaintext metadata object stored on the backend as
/// `meta/.encryption-verifier`, next to the notes themselves.
///
/// It carries the Argon2id salt (so every device derives the same key) and
/// an encrypted copy of [`VERIFIER_PLAINTEXT`] (so a wrong password is
/// detected before any note traffic, instead of failing note by note). It
/// is deliberately not secret: it exists so the *password* can be checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verifier {
    /// Argon2id salt, hex-encoded.
    pub salt: String,
    /// [`VERIFIER_PLAINTEXT`] encrypted under the derived key, hex-encoded.
    pub check: String,
}

impl Cipher {
    /// Derive the key from a password and salt via Argon2id.
    ///
    /// Devices must pass the **same salt** (read from the backend's
    /// verifier or from an object header) to end up with the same key.
    ///
    /// # Errors
    ///
    /// Returns an error if the Argon2id parameters or the derivation
    /// itself fail (a misconfiguration or a host that refuses the
    /// requested memory cost).
    pub fn derive(password: &str, salt: [u8; SALT_LEN]) -> Result<Self, String> {
        let key = derive_key(password, &salt)?;
        Ok(Self { key, salt })
    }

    /// Derive the key from a password and a fresh random salt.
    ///
    /// Used when a backend has never been encrypted: the salt is persisted
    /// via the verifier so later syncs (and other devices) reuse it.
    ///
    /// # Errors
    ///
    /// Same as [`Cipher::derive`].
    pub fn generate(password: &str) -> Result<Self, String> {
        let mut salt = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        Self::derive(password, salt)
    }

    /// The salt this cipher was derived from.
    #[must_use]
    pub fn salt(&self) -> [u8; SALT_LEN] {
        self.salt
    }

    /// Seal a plaintext into a self-describing encrypted blob.
    ///
    /// # Errors
    ///
    /// Returns an error only if the AEAD refuses the input (never for
    /// valid input — the random nonce is drawn from the OS RNG).
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        // A fresh random nonce per object: with 192-bit nonces, random
        // nonce collisions are astronomically unlikely (and, unlike GCM,
        // not catastrophic to the key even if they happened).
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&self.key));
        let sealed = cipher
            .encrypt(XNonce::from_slice(&nonce), plaintext)
            .map_err(|e| format!("encryption failed: {e}"))?;

        let mut blob = Vec::with_capacity(HEADER_LEN + sealed.len());
        blob.extend_from_slice(MAGIC);
        blob.push(VERSION);
        blob.extend_from_slice(&self.salt);
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&sealed);
        Ok(blob)
    }

    /// Open a blob produced by [`Cipher::encrypt`].
    ///
    /// Fails on anything that is not a notas blob, on an unsupported
    /// version, and — thanks to the authentication tag — on any blob
    /// sealed under a different key (wrong password) or tampered with.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed blob (bad magic or version), a
    /// blob too short to carry a header, or an authentication failure
    /// (wrong password, tampering, or data encrypted under another key).
    pub fn decrypt(&self, blob: &[u8]) -> Result<Vec<u8>, String> {
        if blob.len() < HEADER_LEN {
            return Err("not an encrypted notas blob (too short)".to_string());
        }
        if &blob[..MAGIC.len()] != MAGIC {
            return Err("not an encrypted notas blob (missing magic)".to_string());
        }
        if blob[MAGIC.len()] != VERSION {
            return Err(format!(
                "unsupported encrypted blob version {}",
                blob[MAGIC.len()]
            ));
        }
        let nonce = &blob[MAGIC.len() + 1 + SALT_LEN..HEADER_LEN];
        let sealed = &blob[HEADER_LEN..];
        let cipher = XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&self.key));
        cipher
            .decrypt(XNonce::from_slice(nonce), sealed)
            .map_err(|_| {
                "decryption failed: wrong encryption password or corrupted data".to_string()
            })
    }

    /// Build the verifier object for this cipher's salt and key.
    ///
    /// # Errors
    ///
    /// Same as [`Cipher::encrypt`].
    pub fn verifier(&self) -> Result<Verifier, String> {
        let check = self.encrypt(VERIFIER_PLAINTEXT)?;
        Ok(Verifier {
            salt: hex::encode(self.salt),
            check: hex::encode(check),
        })
    }

    /// Constant-time check that a stored verifier matches this cipher:
    /// the verifier's check value must decrypt and equal the known
    /// plaintext. `false` means the password is wrong.
    #[must_use]
    pub fn verify(&self, verifier: &Verifier) -> bool {
        let Ok(check) = hex::decode(&verifier.check) else {
            return false;
        };
        let Ok(opened) = self.decrypt(&check) else {
            return false;
        };
        opened.len() == VERIFIER_PLAINTEXT.len() && bool::from(opened.ct_eq(VERIFIER_PLAINTEXT))
    }
}

impl Verifier {
    /// Decode the hex-encoded salt into bytes for [`Cipher::derive`].
    ///
    /// The salt length lives in this module, so callers never duplicate
    /// the constant.
    ///
    /// # Errors
    ///
    /// Returns an error when the salt is not valid hex or not exactly
    /// [`SALT_LEN`] bytes (a verifier written by a different version, or
    /// a corrupt/foreign verifier object).
    pub fn salt_bytes(&self) -> Result<[u8; SALT_LEN], String> {
        let bytes = hex::decode(&self.salt)
            .map_err(|e| format!("the verifier's salt is not valid hex: {e}"))?;
        bytes
            .try_into()
            .map_err(|_| "the verifier's salt has the wrong length".to_string())
    }
}

/// Argon2id key derivation: 32 bytes from password + salt.
fn derive_key(password: &str, salt: &[u8; SALT_LEN]) -> Result<[u8; 32], String> {
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(32))
        .map_err(|e| format!("cannot configure Argon2id: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| format!("key derivation failed: {e}"))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "correct horse battery staple";

    fn cipher(password: &str) -> Cipher {
        Cipher::derive(password, [7u8; SALT_LEN]).expect("derive")
    }

    #[test]
    fn roundtrip_encrypt_decrypt() {
        let cipher = cipher(PASSWORD);
        for plaintext in [
            "".as_bytes(),
            b"hello".as_ref(),
            b"a longer markdown body\nwith \xff binary-ish bytes".as_ref(),
        ] {
            let blob = cipher.encrypt(plaintext).expect("encrypt");
            assert_eq!(
                cipher.decrypt(&blob).expect("decrypt"),
                plaintext,
                "roundtrip must restore the exact plaintext"
            );
        }
    }

    #[test]
    fn same_password_and_salt_derive_the_same_key() {
        // The salt is shared (it lives on the backend), so two devices
        // with the same password must end up with the same key.
        let a = cipher(PASSWORD);
        let b = cipher(PASSWORD);
        let blob = a.encrypt(b"shared").expect("encrypt");
        assert_eq!(b.decrypt(&blob).expect("decrypt"), b"shared");
    }

    #[test]
    fn different_salt_derives_a_different_key() {
        let a = Cipher::derive(PASSWORD, [1u8; SALT_LEN]).expect("a");
        let b = Cipher::derive(PASSWORD, [2u8; SALT_LEN]).expect("b");
        let blob = a.encrypt(b"hello").expect("encrypt");
        assert!(b.decrypt(&blob).is_err(), "different salt must fail");
    }

    #[test]
    fn wrong_password_fails_to_decrypt() {
        let blob = cipher(PASSWORD).encrypt(b"secret note").expect("encrypt");
        let err = cipher("wrong password").decrypt(&blob).unwrap_err();
        assert!(
            err.contains("password") || err.contains("decryption"),
            "{err}"
        );
    }

    #[test]
    fn tampered_blob_fails_to_decrypt() {
        let mut blob = cipher(PASSWORD).encrypt(b"secret note").expect("encrypt");
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        assert!(cipher(PASSWORD).decrypt(&blob).is_err());
    }

    #[test]
    fn plaintext_and_foreign_bytes_are_rejected() {
        let cipher = cipher(PASSWORD);
        for not_a_blob in [b"{}".as_ref(), b"plaintext".as_ref(), &[0u8; 10]] {
            let err = cipher.decrypt(not_a_blob).unwrap_err();
            assert!(err.contains("magic") || err.contains("short"), "{err}");
        }
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut blob = cipher(PASSWORD).encrypt(b"x").expect("encrypt");
        blob[MAGIC.len()] = VERSION + 1;
        let err = cipher(PASSWORD).decrypt(&blob).unwrap_err();
        assert!(err.contains("version"), "{err}");
    }

    #[test]
    fn verifier_accepts_the_right_password() {
        let cipher = cipher(PASSWORD);
        let verifier = cipher.verifier().expect("verifier");
        assert!(cipher.verify(&verifier));
    }

    #[test]
    fn verifier_rejects_a_wrong_password() {
        let original = cipher(PASSWORD);
        let verifier = original.verifier().expect("verifier");
        assert!(!cipher(&PASSWORD.replace('r', "R")).verify(&verifier));
    }

    #[test]
    fn verifier_salt_roundtrips_through_hex() {
        let cipher = cipher(PASSWORD);
        let verifier = cipher.verifier().expect("verifier");
        let salt = verifier.salt_bytes().expect("salt bytes");
        // The same salt must reproduce the same key, so a verifier written
        // by device A works for device B.
        let other = Cipher::derive(PASSWORD, salt).expect("derive");
        assert!(other.verify(&verifier));
        assert_eq!(salt, cipher.salt());
    }

    #[test]
    fn verifier_salt_rejects_bad_hex_and_wrong_length() {
        let verifier = cipher(PASSWORD).verifier().expect("verifier");
        let mut bad = verifier.clone();
        bad.salt = "not hex!".into();
        assert!(bad.salt_bytes().is_err(), "non-hex salt must fail");
        bad.salt = "abcd".into();
        assert!(bad.salt_bytes().is_err(), "short salt must fail");
    }

    #[test]
    fn a_fresh_salt_is_random() {
        let a = Cipher::generate(PASSWORD).expect("a");
        let b = Cipher::generate(PASSWORD).expect("b");
        assert_ne!(a.salt(), b.salt());
    }
}
