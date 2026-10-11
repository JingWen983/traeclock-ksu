//! Small shared utilities: hex, random identifiers, SHA-256 seeded streams.
//!
//! `seeded_stream` is a byte-for-byte port of the deterministic fingerprint
//! primitive in `TraeWorkAssistant/src-tauri/src/commands/accounts.rs`:
//! SHA-256("salt:seed" + 4-byte big-endian counter), outputs concatenated.
//! Determinism across platforms is the core of the anti-risk-control design —
//! the same account seed MUST always produce the same bytes.

use sha2::{Digest, Sha256};

pub fn to_hex_upper(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02X}", b));
    }
    s
}

pub fn to_hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Local timestamp, "%Y-%m-%dT%H:%M:%S" (matches the source's fs_utils::now_iso style).
pub fn now_iso_local() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// `random_hex(n_chars)` — cryptographically random lowercase hex string of
/// exactly `n` characters (matches the source semantics: random_hex(len)
/// produces len chars from ceil(len/2) random bytes). Source uses
/// random_hex(32) for x-request-id / trace ids, random_hex(64) for PKCE verifiers.
pub fn random_hex(n_chars: usize) -> String {
    use rand::RngCore;
    let n_bytes = n_chars.div_ceil(2);
    let mut buf = vec![0u8; n_bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    let full = to_hex_lower(&buf);
    full[..n_chars].to_string()
}

/// SHA-256 of the UTF-8 input, lowercase hex.
pub fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    to_hex_lower(&h.finalize())
}

/// Byte-for-byte port of the source's deterministic stream:
/// stream(i) = SHA-256(UTF-8("salt:seed") + i as 4-byte BE), outputs concatenated
/// until `nbytes` bytes have been produced.
pub fn seeded_stream(seed: &str, salt: &str, nbytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(nbytes);
    let prefix = format!("{}:{}", salt, seed);
    let mut counter: u32 = 0;
    while out.len() < nbytes {
        let mut h = Sha256::new();
        h.update(prefix.as_bytes());
        h.update(counter.to_be_bytes());
        out.extend_from_slice(&h.finalize());
        counter += 1;
    }
    out.truncate(nbytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_stream_is_deterministic() {
        let a = seeded_stream("uid-1", "devid", 15);
        let b = seeded_stream("uid-1", "devid", 15);
        assert_eq!(a, b);
        assert_eq!(a.len(), 15);
        // different salt => different bytes
        assert_ne!(a, seeded_stream("uid-1", "sess", 15));
        // different seed => different bytes
        assert_ne!(a, seeded_stream("uid-2", "devid", 15));
    }

    #[test]
    fn seeded_stream_spans_multiple_blocks() {
        // 40 bytes > 32-byte single SHA-256 output, must not repeat the first block
        let s = seeded_stream("x", "s", 40);
        assert_eq!(s.len(), 40);
        assert_ne!(&s[..32], &s[8..40]);
    }

    #[test]
    fn sha256_known_vector() {
        // SHA-256("abc")
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn random_hex_shapes() {
        let s = random_hex(32);
        assert_eq!(s.len(), 32);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // odd length works (truncation)
        assert_eq!(random_hex(5).len(), 5);
    }
}
