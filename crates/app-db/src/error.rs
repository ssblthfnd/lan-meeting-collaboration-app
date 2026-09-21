//! Persistence errors.
//!
//! Errors name the expected and the detected value wherever the information
//! exists (architecture rules section 22). A caller that cannot tell a
//! constraint violation from an I/O failure cannot produce a useful message for
//! the Host, so the common integrity failures get their own variants instead of
//! being flattened into one opaque "database error".

use thiserror::Error;

pub type DbResult<T> = Result<T, DbError>;

/// Errors produced by the persistence layer.
#[derive(Debug, Error)]
pub enum DbError {
    /// The database file could not be opened or prepared.
    #[error("could not open the database at {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: rusqlite::Error,
    },

    /// A required PRAGMA did not take effect.
    ///
    /// Worth its own variant: silently running without `foreign_keys` would
    /// leave every referential guarantee in the schema unenforced.
    #[error("PRAGMA {pragma} could not be applied: expected {expected}, detected {detected}")]
    Pragma {
        pragma: &'static str,
        expected: String,
        detected: String,
    },

    /// Schema migration failed.
    #[error("schema migration failed: {0}")]
    Migration(#[from] refinery::Error),

    /// A uniqueness or check constraint rejected the write.
    #[error("constraint violated: {detail}")]
    Constraint { detail: String },

    /// Stored data is not what the schema guarantees about it.
    ///
    /// Reachable only when something wrote past a `CHECK` constraint - a direct
    /// edit of the file, or a future code path that bypassed this crate. Read
    /// back as a failure rather than quietly coerced, because silently showing
    /// such a row as empty would hide the fact that the database is wrong.
    #[error(
        "stored {what} is not what the schema guarantees: expected {expected}, detected {detected}"
    )]
    Malformed {
        what: &'static str,
        expected: String,
        detected: String,
    },

    /// The writer connection was poisoned by a panic in another thread.
    #[error("the database writer is unusable because a previous write panicked")]
    WriterPoisoned,

    /// A connection could not be taken from the read pool.
    #[error("could not obtain a database connection from the pool: {0}")]
    Pool(#[from] r2d2::Error),

    /// Any other SQLite failure.
    #[error("database error: {0}")]
    Sqlite(rusqlite::Error),
}

impl From<rusqlite::Error> for DbError {
    fn from(error: rusqlite::Error) -> Self {
        // Constraint failures are the ones a caller can act on, so they are
        // separated here rather than at every call site.
        if let rusqlite::Error::SqliteFailure(inner, ref message) = error {
            if inner.code == rusqlite::ErrorCode::ConstraintViolation {
                return DbError::Constraint {
                    detail: message.clone().unwrap_or_else(|| inner.to_string()),
                };
            }
        }
        DbError::Sqlite(error)
    }
}

impl DbError {
    /// Whether this error is a constraint violation.
    ///
    /// Used by callers that treat a specific violation as an expected outcome,
    /// such as a duplicate remote submission being a detected duplicate rather
    /// than a failure (ADR-0008).
    #[must_use]
    pub fn is_constraint_violation(&self) -> bool {
        matches!(self, DbError::Constraint { .. })
    }
}
