//! Credential hashes.
//!
//! Two credentials cross the LAN: the **join token**, which proves someone was
//! given the meeting's URL or QR code, and the **session token**, which proves
//! someone already claimed an identity. Neither is ever stored. What is stored
//! is a [`TokenHash`], and this type exists so that "only the hash is persisted"
//! is a property of the type system rather than a rule to remember
//! (PRD 22.2, 22.17; ADR-0002 rules 7 and 9).
//!
//! # Why the domain holds the hash but not the hashing
//!
//! `app-core` has no `sha2` and no `rand`. Generating entropy and hashing are
//! transport concerns - the LAN server does both - and a domain that could hash
//! could also be handed a plaintext token by a careless caller. Here the only
//! thing that can be constructed is a *hash*, and the only way to construct one
//! is to parse 64 lowercase hexadecimal characters. A plaintext token is not a
//! `TokenHash` and will not parse as one, so a write method that takes a
//! `TokenHash` cannot be passed a secret by mistake.
//!
//! The format mirrors the `CHECK (length(...) = 64 AND NOT ... GLOB
//! '*[^0-9a-f]*')` constraints already in the migration, which is the second,
//! independent enforcement point ADR-0011 asks for.

use core::fmt;
use core::str::FromStr;

use thiserror::Error;

/// Characters in the canonical stored form of a SHA-256 digest.
pub const TOKEN_HASH_WIDTH: usize = 64;

/// Why a value could not be a credential hash.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TokenError {
    #[error(
        "invalid credential hash: expected {TOKEN_HASH_WIDTH} characters, detected {detected}"
    )]
    Width { detected: usize },

    /// Deliberately does **not** echo the offending value.
    ///
    /// Everywhere else an error names the detected value, because that is what
    /// makes it actionable (architecture rules section 22). Not here: the only
    /// thing that is nearly a token hash is a token, and an error message is
    /// the last place a secret should be copied into.
    #[error("invalid credential hash: expected lowercase hexadecimal characters only")]
    NotLowercaseHex,
}

/// The SHA-256 digest of a credential, as stored.
///
/// Holds the canonical 64-character lowercase hexadecimal form. Comparison is
/// by value: a lookup is an indexed equality match on this column, so the
/// database finds the row without anything having to compare secrets.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TokenHash(String);

impl TokenHash {
    /// Parse the canonical stored form.
    pub fn parse(text: &str) -> Result<Self, TokenError> {
        if text.len() != TOKEN_HASH_WIDTH {
            return Err(TokenError::Width {
                detected: text.len(),
            });
        }
        if !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(TokenError::NotLowercaseHex);
        }
        Ok(Self(text.to_owned()))
    }

    /// The canonical form written to SQLite.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The canonical form, owned.
    #[must_use]
    pub fn to_storage(&self) -> String {
        self.0.clone()
    }
}

impl FromStr for TokenHash {
    type Err = TokenError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Prints the hash, which is not a secret. The token it came from is.
impl fmt::Display for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "3a7bd3e2360a3d29eea436fcfb7e44c735d117c42d1c1835420b6b9942dd4f1b";

    #[test]
    fn the_canonical_form_round_trips() {
        let hash = TokenHash::parse(VALID).unwrap();
        assert_eq!(hash.as_str(), VALID);
        assert_eq!(hash.to_storage(), VALID);
        assert_eq!(VALID.parse::<TokenHash>().unwrap(), hash);
    }

    #[test]
    fn anything_that_is_not_a_lowercase_hex_digest_is_refused() {
        // Too short, too long, uppercase, and non-hex.
        assert_eq!(
            TokenHash::parse("abc"),
            Err(TokenError::Width { detected: 3 })
        );
        assert!(matches!(
            TokenHash::parse(&format!("{VALID}0")),
            Err(TokenError::Width { .. })
        ));
        assert_eq!(
            TokenHash::parse(&VALID.to_uppercase()),
            Err(TokenError::NotLowercaseHex)
        );
        assert_eq!(
            TokenHash::parse(&"g".repeat(TOKEN_HASH_WIDTH)),
            Err(TokenError::NotLowercaseHex)
        );
    }

    #[test]
    fn a_rejection_never_echoes_the_value_it_rejected() {
        // The likeliest thing to be mistakenly parsed as a hash is a token, and
        // an error message is the last place a secret should end up.
        let secret = "S3CRET-token-value-that-must-never-appear-in-any-log-message!!!!";
        assert_eq!(secret.len(), TOKEN_HASH_WIDTH);

        let message = TokenHash::parse(secret).unwrap_err().to_string();
        assert!(!message.contains("S3CRET"), "{message}");
        assert!(!message.contains(secret), "{message}");
    }

    #[test]
    fn a_plaintext_token_is_not_a_hash() {
        // The point of the type: a 64-character random token in the wrong
        // encoding cannot be handed to a write method expecting a hash.
        assert!(
            TokenHash::parse("Zm9vYmFyYmF6cXV1eHF1dXhmb29iYXJiYXpxdXV4cXV1eGZvb2Jhcg==").is_err()
        );
    }
}
