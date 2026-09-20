//! Normalisation of Host-supplied text fields.
//!
//! Two rules, applied in one place so every command treats text the same way:
//!
//! - A **required** field must contain at least one non-whitespace character,
//!   which is what the `length(trim(...)) > 0` checks in the migration also
//!   demand. It is stored trimmed.
//! - An **optional** field that is absent, empty or whitespace is stored as
//!   `NULL` rather than as an empty string. Without this, "no department" would
//!   have three indistinguishable representations, and exports would have to
//!   guess which of them means absent.

use crate::error::{DomainError, DomainResult};

/// Trim a required field, refusing it when nothing is left.
pub(crate) fn required(field: &'static str, value: &str) -> DomainResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(DomainError::Validation {
            field,
            expected: "at least one non-whitespace character",
            detected: if value.is_empty() {
                "an empty string".to_owned()
            } else {
                "only whitespace".to_owned()
            },
        });
    }
    Ok(trimmed.to_owned())
}

/// Trim an optional field, collapsing "blank" to absent.
pub(crate) fn optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_required_field_is_stored_trimmed() {
        assert_eq!(
            required("title", "  Weekly Coordination  ").unwrap(),
            "Weekly Coordination"
        );
    }

    #[test]
    fn a_required_field_that_is_blank_names_what_was_detected() {
        let empty = required("title", "").unwrap_err();
        assert!(empty.to_string().contains("an empty string"), "{empty}");

        let blank = required("title", "  \t").unwrap_err();
        assert!(blank.to_string().contains("only whitespace"), "{blank}");
    }

    #[test]
    fn a_blank_optional_field_becomes_absent_rather_than_empty() {
        assert_eq!(optional(None), None);
        assert_eq!(optional(Some("")), None);
        assert_eq!(optional(Some("   ")), None);
        assert_eq!(optional(Some("  Finance  ")), Some("Finance".to_owned()));
    }
}
