//! Binding domain types to SQLite.
//!
//! `app-core` deliberately has no SQLite dependency, and `rusqlite`'s `ToSql`
//! and `FromSql` are foreign traits, so they cannot be implemented directly for
//! the foreign `Id<E>` and `UtcTimestamp`. [`Sql`] is a local newtype that
//! makes those impls legal without pulling SQL into the domain crate.
//!
//! Every conversion here goes through the canonical storage form defined in
//! `app-core`, which is the same form the migration's `CHECK` constraints
//! enforce and the same form JSON carries. There is one representation, not
//! three that must be kept in step.

use app_core::id::{Entity, Id};
use app_core::time::{MeetingDate, MeetingTime, UtcTimestamp};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::ToSql;

/// Adapter that lets a domain type be bound to, and read from, SQLite.
///
/// ```ignore
/// stmt.execute((Sql(meeting_id), Sql(created_at)))?;
/// let id: Sql<MeetingId> = row.get(0)?;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Sql<T>(pub T);

impl<T> Sql<T> {
    /// Unwrap the domain value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<E: Entity> ToSql for Sql<Id<E>> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.to_storage()))
    }
}

impl<E: Entity> FromSql for Sql<Id<E>> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        Id::<E>::parse(text)
            .map(Sql)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

impl ToSql for Sql<UtcTimestamp> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.to_storage()))
    }
}

impl FromSql for Sql<UtcTimestamp> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        UtcTimestamp::parse_storage(text)
            .map(Sql)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

impl ToSql for Sql<MeetingDate> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.to_storage()))
    }
}

impl FromSql for Sql<MeetingDate> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        MeetingDate::parse_storage(text)
            .map(Sql)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

impl ToSql for Sql<MeetingTime> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.to_storage()))
    }
}

impl FromSql for Sql<MeetingTime> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        MeetingTime::parse_storage(text)
            .map(Sql)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}
