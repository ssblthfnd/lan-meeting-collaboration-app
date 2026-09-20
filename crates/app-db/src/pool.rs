//! Connection management, PRAGMAs and transactions.
//!
//! # Why one writer and a pool of readers
//!
//! SQLite serialises writes regardless of how many connections exist, so a
//! write pool buys nothing and costs `SQLITE_BUSY` races. The shape here is the
//! one ADR-0006 committed to: a single writer connection behind a mutex, plus a
//! pool for reads, which WAL lets proceed concurrently with the writer.
//!
//! Every write goes through [`Db::write`], which opens an `IMMEDIATE`
//! transaction. Taking the write lock up front avoids the deferred-transaction
//! upgrade deadlock, where two connections both read, then both try to become
//! writers and one is killed.
//!
//! That single entry point is also where the meeting lock will be re-read in a
//! later step: architecture rules section 15 requires the status check to
//! happen *inside* the mutating transaction, and there is exactly one place
//! here for it to live.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use crate::error::{DbError, DbResult};

/// How long a blocked connection waits for the write lock before giving up.
const BUSY_TIMEOUT_MS: u32 = 5_000;

/// Size of the read pool.
///
/// A meeting is capped at 99 participants (PRD section 7) and reads are short,
/// so this is deliberately small.
const READ_POOL_SIZE: u32 = 8;

/// The SQLite database: one writer, a pool of readers.
///
/// The file is local to the Host device and is never exposed to the network
/// (PRD 22.7, 22.8). Nothing here opens a port.
pub struct Db {
    path: PathBuf,
    writer: Mutex<Connection>,
    readers: Pool<SqliteConnectionManager>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").field("path", &self.path).finish()
    }
}

/// Apply the PRAGMAs every connection needs.
///
/// These are per-connection, not per-database, with the exception of
/// `journal_mode`, which is persistent. Applying them on every connection is
/// what makes that distinction stop mattering.
fn configure(conn: &Connection, is_writer: bool) -> DbResult<()> {
    // Referential integrity is off by default in SQLite. Every foreign key in
    // the schema depends on this being on, for this connection.
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_millis(u64::from(BUSY_TIMEOUT_MS)))?;

    if is_writer {
        // WAL lets readers run while a write is in progress, which is the whole
        // reason the read pool is useful. It is persistent, so the writer sets
        // it once for the database.
        let mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(DbError::Pragma {
                pragma: "journal_mode",
                expected: "wal".to_owned(),
                detected: mode,
            });
        }
    }

    // NORMAL is the correct durability setting under WAL: a crash cannot
    // corrupt the database, and the data at risk is the last transaction.
    conn.pragma_update(None, "synchronous", "NORMAL")?;

    verify_pragma(conn, "foreign_keys", 1)?;
    Ok(())
}

/// Read a PRAGMA back and confirm it took.
fn verify_pragma(conn: &Connection, pragma: &'static str, expected: i64) -> DbResult<()> {
    let detected: i64 = conn.pragma_query_value(None, pragma, |row| row.get(0))?;
    if detected != expected {
        return Err(DbError::Pragma {
            pragma,
            expected: expected.to_string(),
            detected: detected.to_string(),
        });
    }
    Ok(())
}

impl Db {
    /// Open (creating if absent) the database at `path`, apply PRAGMAs and run
    /// every outstanding migration.
    ///
    /// Migrations run on the writer connection inside refinery's own
    /// transaction, so a failure leaves the schema untouched.
    pub fn open(path: impl AsRef<Path>) -> DbResult<Self> {
        let path = path.as_ref().to_path_buf();

        let mut writer = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|source| DbError::Open {
            path: path.display().to_string(),
            source,
        })?;

        configure(&writer, true)?;
        crate::migrations::run(&mut writer)?;

        let manager = SqliteConnectionManager::file(&path)
            .with_flags(
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_URI
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .with_init(|conn| {
                configure(conn, false).map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
                        Some(e.to_string()),
                    )
                })
            });

        let readers = Pool::builder().max_size(READ_POOL_SIZE).build(manager)?;

        Ok(Self {
            path,
            writer: Mutex::new(writer),
            readers,
        })
    }

    /// Path of the database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn writer(&self) -> DbResult<MutexGuard<'_, Connection>> {
        self.writer.lock().map_err(|_| DbError::WriterPoisoned)
    }

    /// Run `f` inside a single `IMMEDIATE` write transaction.
    ///
    /// The transaction commits when `f` returns `Ok` and rolls back on `Err`,
    /// so a partial write is not representable. Import in particular must be
    /// all-or-nothing (architecture rules section 11).
    pub fn write<T, F>(&self, f: F) -> DbResult<T>
    where
        F: FnOnce(&Transaction<'_>) -> DbResult<T>,
    {
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    /// Run `f` on a pooled read-only connection.
    ///
    /// The connections are opened `SQLITE_OPEN_READ_ONLY`, so a write attempted
    /// through this path fails rather than bypassing [`Db::write`].
    pub fn read<T, F>(&self, f: F) -> DbResult<T>
    where
        F: FnOnce(&Connection) -> DbResult<T>,
    {
        let conn = self.readers.get()?;
        f(&conn)
    }

    /// Run `f` on the writer connection without opening a transaction.
    ///
    /// For maintenance statements that cannot run inside one, such as
    /// `PRAGMA integrity_check`.
    pub fn with_writer<T, F>(&self, f: F) -> DbResult<T>
    where
        F: FnOnce(&Connection) -> DbResult<T>,
    {
        let conn = self.writer()?;
        f(&conn)
    }

    /// Ask SQLite to verify the database's internal consistency.
    pub fn integrity_check(&self) -> DbResult<String> {
        self.with_writer(|conn| {
            let result: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
            Ok(result)
        })
    }
}
