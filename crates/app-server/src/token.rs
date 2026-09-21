//! Generating and hashing the two LAN credentials.
//!
//! This is the only module in the workspace that ever holds a token in plain
//! text, and it holds one for as long as it takes to hash it and hand it to the
//! caller. Nothing here writes to a database, and nothing that writes to a
//! database can accept what this produces: `app-core` takes only a
//! [`TokenHash`], which a token will not parse as.
//!
//! # Shape
//!
//! 32 bytes from the operating system's CSPRNG, rendered as 64 lowercase
//! hexadecimal characters. 256 bits is beyond guessing, which is the whole
//! defence for a value that travels in a URL on a plain-HTTP LAN - the join
//! URL is an operational secret and the architecture says so (ADR-0002).
//!
//! A password hash would be the wrong tool here and ADR-0006 says why: these are
//! full-entropy random values, not user-chosen secrets, so there is nothing for
//! a slow KDF to defend against. SHA-256 of a 256-bit random value cannot be
//! usefully brute-forced or rainbow-tabled.
//!
//! # Why the plaintext is returned once and never stored
//!
//! [`Credential`] carries both halves. The caller sends `token` to the browser
//! and passes `hash` to the domain; the plaintext is then dropped. There is no
//! path by which it can be persisted, because the persistence layer's signature
//! will not accept it (PRD 22.2, 22.17).

use core::fmt;

use app_core::token::TokenHash;
use rand::TryRngCore;
use sha2::{Digest, Sha256};

/// Bytes of entropy in a token. 256 bits.
const TOKEN_BYTES: usize = 32;

/// A freshly minted credential: the secret, and what will be stored.
///
/// [`Debug`] is implemented by hand rather than derived, so that formatting one
/// cannot print the secret. See the impl below.
#[derive(Clone)]
pub struct Credential {
    /// The plaintext. Goes to exactly one browser, exactly once.
    pub token: String,
    /// What is persisted.
    pub hash: TokenHash,
}

/// Redacts the plaintext token.
///
/// A derived `Debug` would print it, and `{:?}` is how a value ends up in a
/// panic message, a `dbg!` left in during debugging, or an error wrapped by a
/// crate that formats its source. None of those paths should be able to reveal
/// a bearer credential, and the surest way to guarantee that is for there to be
/// no formatting that prints it.
///
/// The hash is shown: it is what the database already holds, it cannot be
/// presented as a credential, and it is what makes a diagnostic useful.
impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("token", &"<redacted>")
            .field("hash", &self.hash)
            .finish()
    }
}

/// Mint a credential from operating-system entropy.
///
/// Fails only if the OS entropy source itself fails, which is not a condition
/// to paper over: a token from a degraded source is worse than no token, so the
/// caller is told rather than handed something weak.
pub fn mint() -> Result<Credential, TokenGenerationError> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|error| TokenGenerationError {
            detail: error.to_string(),
        })?;

    let token = to_hex(&bytes);
    let hash = hash_token(&token);
    Ok(Credential { token, hash })
}

/// The stored form of a presented token.
///
/// Total by construction: any string hashes to a valid 64-character lowercase
/// digest, so a malformed or hostile token produces a hash that simply matches
/// no row. There is no parse step here for an attacker to probe.
#[must_use]
pub fn hash_token(token: &str) -> TokenHash {
    let digest = Sha256::digest(token.as_bytes());
    TokenHash::parse(&to_hex(&digest))
        .expect("a SHA-256 digest is always 64 lowercase hexadecimal characters")
}

/// Lowercase hexadecimal, the canonical storage form (ADR-0011).
fn to_hex(bytes: &[u8]) -> String {
    use core::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// The operating system could not supply entropy.
#[derive(Debug, thiserror::Error)]
#[error("could not generate a secure token: {detail}")]
pub struct TokenGenerationError {
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_minted_token_is_256_bits_of_hex() {
        let credential = mint().expect("entropy");
        assert_eq!(credential.token.len(), TOKEN_BYTES * 2);
        assert!(credential
            .token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    #[test]
    fn the_hash_is_of_the_token_and_is_not_the_token() {
        let credential = mint().expect("entropy");
        assert_eq!(credential.hash, hash_token(&credential.token));
        // The distinction the whole design rests on.
        assert_ne!(credential.hash.as_str(), credential.token);
    }

    #[test]
    fn minting_does_not_repeat() {
        // Not a statistical test - with 256 bits a collision here would mean
        // the entropy source is broken, which is exactly what it would catch.
        let mut seen = HashSet::new();
        for _ in 0..512 {
            assert!(
                seen.insert(mint().expect("entropy").token),
                "repeated token"
            );
        }
    }

    #[test]
    fn hashing_is_deterministic_and_total() {
        // Any input hashes, including the ones a hostile client would send, so
        // an unknown credential is a lookup that matches nothing rather than a
        // parse failure that behaves differently.
        for hostile in [
            "",
            "../../etc/passwd",
            "'; DROP TABLE meetings; --",
            "ünïcøde",
        ] {
            let once = hash_token(hostile);
            assert_eq!(once, hash_token(hostile));
            assert_eq!(once.as_str().len(), 64);
        }
    }

    #[test]
    fn debug_formatting_never_reveals_the_issued_token() {
        // The one place a bearer credential could escape without anyone
        // meaning to: a `{:?}` in a log line, a panic message, or a `dbg!`
        // left behind. There must be no formatting that prints it.
        let credential = mint().expect("entropy");

        let debugged = format!("{credential:?}");
        assert!(
            !debugged.contains(&credential.token),
            "the plaintext token leaked through Debug: {debugged}"
        );
        assert!(debugged.contains("<redacted>"), "{debugged}");

        // The hash is not a credential and stays visible, so a diagnostic can
        // still be matched against the stored row.
        assert!(debugged.contains(credential.hash.as_str()), "{debugged}");

        // Nesting must not reintroduce it: a `Result` or an `Option` formats
        // its contents with the same impl.
        let wrapped = format!("{:?}", Some(&credential));
        assert!(!wrapped.contains(&credential.token), "{wrapped}");
    }

    #[test]
    fn a_known_vector_matches_sha256() {
        // Pins the algorithm: if this changes, every stored hash stops matching
        // and that must be a deliberate migration rather than a silent break.
        assert_eq!(
            hash_token("abc").as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
