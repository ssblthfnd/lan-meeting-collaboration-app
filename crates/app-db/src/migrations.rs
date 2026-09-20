//! Embedded schema migrations.
//!
//! The `.sql` files under `migrations/` are compiled into the binary, so a Host
//! machine needs no migration files on disk and no network access to upgrade
//! its database (PRD section 4, architecture rules section 5).
//!
//! Migrations are append-only: an applied migration is never edited, because
//! refinery records a checksum and will refuse a file that changed after it ran.
//! A correction is a new `V{n}__*.sql`.

use refinery::Report;
use rusqlite::Connection;

use crate::error::DbResult;

mod embedded {
    refinery::embed_migrations!("migrations");
}

/// Apply every migration that has not yet run.
///
/// refinery wraps the run in a transaction and records applied versions in its
/// own `refinery_schema_history` table, so this is safe to call on every start
/// and is a no-op once the schema is current.
pub fn run(conn: &mut Connection) -> DbResult<Report> {
    let report = embedded::migrations::runner().run(conn)?;
    Ok(report)
}

/// The migration versions embedded in this binary.
#[must_use]
pub fn embedded_versions() -> Vec<refinery::SchemaVersion> {
    embedded::migrations::runner()
        .get_migrations()
        .iter()
        .map(refinery::Migration::version)
        .collect()
}
