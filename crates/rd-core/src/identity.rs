//! Device identity.
//!
//! Every installation generates a random 32-byte secret and keeps it private.
//! Its public device ID is a hash of that secret, so the server can check that
//! a device really owns the ID it registers with, without storing anything.
//!
//! Keep this in sync with `src/identity.rs` in the nat-server repository.

use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const SECRET_LEN: usize = 32;
const DOMAIN: &[u8] = b"delickate-remote-access/device-id/v1";

/// Returns the device ID for a hex-encoded secret, or `None` if the secret is
/// not exactly 32 bytes of valid hex.
pub fn device_id_from_secret(secret_hex: &str) -> Option<Uuid> {
    let secret = hex::decode(secret_hex.trim()).ok()?;
    if secret.len() != SECRET_LEN {
        return None;
    }
    let digest = Sha256::new()
        .chain_update(DOMAIN)
        .chain_update(&secret)
        .finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Some(uuid::Builder::from_random_bytes(bytes).into_uuid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_deterministic_and_secret_specific() {
        let a = "11".repeat(32);
        let b = "22".repeat(32);
        assert_eq!(device_id_from_secret(&a), device_id_from_secret(&a));
        assert_ne!(device_id_from_secret(&a), device_id_from_secret(&b));
        assert_eq!(device_id_from_secret(&a).unwrap().get_version_num(), 4);
    }

    #[test]
    fn known_vector() {
        // Pinned so the client and server derivations can be checked against each other.
        let id = device_id_from_secret(&"00".repeat(32)).unwrap();
        assert_eq!(id.to_string(), KNOWN_ID);
    }
    const KNOWN_ID: &str = "2242bd17-c73b-4434-875f-d53a26b84777";

    #[test]
    fn bad_secrets_are_rejected() {
        assert!(device_id_from_secret("").is_none());
        assert!(device_id_from_secret("zz").is_none());
        assert!(device_id_from_secret(&"00".repeat(31)).is_none());
        assert!(device_id_from_secret(&"00".repeat(33)).is_none());
    }
}
