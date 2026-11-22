//! Ed25519 signatures for OTA images.
//!
//! The signed message is `SIGNATURE_CONTEXT || sha256(image)`. This must match
//! `ull_esp_core::signature` in ull-esp-rs, which verifies on the device; the
//! shared test vector below keeps the two in step.

pub mod appdesc;

use anyhow::{Context, bail};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

pub const SIGNATURE_CONTEXT: &[u8; 20] = b"ull-esp-ota-image-v1";
/// HTTP header that carries the signature, as 128 hex digits.
pub const SIGNATURE_HEADER: &str = "x-application-image-ed25519";

pub fn sha256(image: &[u8]) -> [u8; 32] {
    Sha256::digest(image).into()
}

pub fn signed_message(sha256: &[u8; 32]) -> Vec<u8> {
    [SIGNATURE_CONTEXT.as_slice(), sha256].concat()
}

pub fn sign_image(secret_seed: &[u8; 32], image: &[u8]) -> [u8; 64] {
    SigningKey::from_bytes(secret_seed)
        .sign(&signed_message(&sha256(image)))
        .to_bytes()
}

pub fn public_key(secret_seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(secret_seed)
        .verifying_key()
        .to_bytes()
}

/// Same checks as the device: strict verification and no weak keys.
pub fn verify_image(
    public_key: &[u8; 32],
    image: &[u8],
    signature: &[u8; 64],
) -> anyhow::Result<()> {
    let key = VerifyingKey::from_bytes(public_key).context("invalid Ed25519 public key")?;
    if key.is_weak() {
        bail!("weak Ed25519 public key");
    }
    key.verify_strict(
        &signed_message(&sha256(image)),
        &Signature::from_bytes(signature),
    )
    .context("signature does not match the image and public key")
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn from_hex<const N: usize>(value: &str) -> anyhow::Result<[u8; N]> {
    let value = value.trim();
    if value.len() != N * 2 {
        bail!(
            "expected {} hex digits, got {} characters",
            N * 2,
            value.len()
        );
    }
    if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("expected only hex digits");
    }
    let mut bytes = [0u8; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same vector as `ull_esp_core::signature::tests::cross_repo_test_vector`,
    /// generated independently with Python `cryptography`. There the message
    /// digest is `[0xab; 32]`; here we sign that digest's message directly.
    #[test]
    fn cross_repo_test_vector() {
        let seed = [7u8; 32];
        let signature = SigningKey::from_bytes(&seed)
            .sign(&signed_message(&[0xab; 32]))
            .to_bytes();
        assert_eq!(
            to_hex(&public_key(&seed)),
            "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c"
        );
        assert_eq!(
            to_hex(&signature),
            "c1c2fa02a0e90648084724d2ad795a73132e04f18cebe93f1e690135382afcc9\
             5a6c2ac63b3e34b389735177051775a03b6e1c18d7cf53d57e9d8a15d300b502"
        );
    }

    #[test]
    fn sign_then_verify() {
        let seed = [3u8; 32];
        let image = b"firmware".as_slice();
        let signature = sign_image(&seed, image);
        verify_image(&public_key(&seed), image, &signature).unwrap();
        assert!(verify_image(&public_key(&seed), b"firmwarf", &signature).is_err());
        assert!(verify_image(&public_key(&[4; 32]), image, &signature).is_err());
    }

    #[test]
    fn hex_round_trip_and_errors() {
        let bytes = [0x00, 0x7f, 0xff];
        assert_eq!(from_hex::<3>(&to_hex(&bytes)).unwrap(), bytes);
        assert_eq!(from_hex::<3>(" 007FFF\n").unwrap(), bytes);
        assert!(from_hex::<3>("007f").is_err());
        assert!(from_hex::<3>("007fzz").is_err());
        assert!(from_hex::<2>("+1ff").is_err());
        assert!(from_hex::<2>("éé").is_err());
    }
}
