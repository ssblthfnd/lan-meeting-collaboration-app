//! The artefact a Host has chosen, before they have confirmed it.
//!
//! Step 10's answer to a small but load-bearing question: **where does the
//! submission live between preview and confirmation?**
//!
//! Not in the renderer. A window that could hand back "the artefact" at confirm
//! time could hand back one the Host never saw, and the preview they read would
//! have decided nothing. So the bytes stay here, in Rust, and the confirm
//! command takes a meeting id and nothing else (ADR-0022 decision 4).
//!
//! Not a path, either. Remembering only where the file was and reading it again
//! at confirm time would import whatever the file says *then* - which is not
//! what was previewed. The exact bounded bytes are kept.
//!
//! # Why the renderer never names a path
//!
//! A dropped file's path arrives from Tauri's own drag-drop event, in Rust. The
//! window is told a file is waiting and calls preview with a meeting id; it has
//! no way to say *which* file, because there is no parameter for it. A renderer
//! that cannot name a path cannot read an arbitrary one - the same principle
//! step 9 applied to writing (ADR-0021 decision 10).
//!
//! Pasted JSON is different in kind and the same in handling: the text is data
//! rather than a filesystem path, so it may arrive from the window, but it is
//! bounded and stored here exactly as a dropped file would be, and confirmation
//! reads it from here.
//!
//! # One slot
//!
//! A Host imports one submission at a time, and the application has one window.
//! A single slot is the whole mechanism; keying it would be infrastructure for a
//! situation that does not exist.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use app_remote::{RemoteError, MAX_SUBMISSION_BYTES};

use tauri::{Emitter, Manager};

use crate::error::{HostError, HostErrorKind, HostResult};

/// The window event announcing that a file was dropped and is now pending.
///
/// Carries only what the Host should see - the origin and the size - so the
/// window learns that something arrived without learning where it came from on
/// disk. The renderer's next move is to call preview with a meeting id.
pub const REMOTE_SUBMISSION_PENDING: &str = "remote-submission-pending";

/// What is waiting to be previewed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingArtifact {
    /// The exact submitted text, bounded and verified as UTF-8.
    ///
    /// Stored verbatim: this is what `remote_submissions.raw_payload` keeps, and
    /// re-serialising it would lose the forensic record (ADR-0022 decision 15).
    pub raw: String,
    /// Where it came from, for the Host to recognise. Never an identity, and
    /// never parsed (architecture rules section 21).
    pub origin: String,
}

/// The one pending slot, managed by Tauri for the life of the application.
#[derive(Debug, Default)]
pub struct PendingImport {
    slot: Mutex<Option<PendingArtifact>>,
}

impl PendingImport {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a dropped file into the slot, bounded.
    ///
    /// Returns what the Host should be shown about it. The file's extension is
    /// a hint and nothing more: a `.json` name does not make malformed JSON
    /// valid, and the content is parsed against the frozen contract either way.
    pub fn accept_file(&self, path: &Path) -> HostResult<PendingArtifact> {
        let raw = read_bounded(path)?;
        let origin = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );

        Ok(self.store(PendingArtifact { raw, origin }))
    }

    /// Take pasted text into the slot, bounded by the same limit.
    pub fn accept_text(&self, raw: String) -> HostResult<PendingArtifact> {
        if raw.len() > MAX_SUBMISSION_BYTES {
            return Err(HostError::from(RemoteError::TooLarge {
                limit: MAX_SUBMISSION_BYTES,
                detected: raw.len() as u64,
            }));
        }

        Ok(self.store(PendingArtifact {
            raw,
            origin: "pasted text".to_owned(),
        }))
    }

    /// What is waiting, if anything.
    #[must_use]
    pub fn peek(&self) -> Option<PendingArtifact> {
        self.lock().clone()
    }

    /// What is waiting, or an actionable refusal.
    ///
    /// Used by preview and by confirmation, so both act on the same bytes.
    pub fn require(&self) -> HostResult<PendingArtifact> {
        self.peek().ok_or_else(|| {
            HostError::new(
                HostErrorKind::Validation {
                    field: "submission".to_owned(),
                    expected: "a submission file dropped on this window, or pasted text".to_owned(),
                    detected: "nothing selected".to_owned(),
                },
                "No submission is waiting. Drop a submission file on this window, \
                 or paste one, and try again."
                    .to_owned(),
            )
        })
    }

    /// Forget whatever was waiting.
    ///
    /// Called after a successful import and whenever the Host cancels, so a
    /// confirmed artefact cannot be confirmed twice by accident. It is not a
    /// security control - the ledger is, and it is checked inside the
    /// transaction - but it keeps the window honest about what is pending.
    pub fn clear(&self) {
        *self.lock() = None;
    }

    fn store(&self, artifact: PendingArtifact) -> PendingArtifact {
        let mut slot = self.lock();
        *slot = Some(artifact.clone());
        artifact
    }

    /// The slot, recovering from a poisoned lock rather than panicking.
    ///
    /// A panic while holding this mutex would otherwise make importing
    /// impossible for the rest of the session, and there is no invariant here
    /// that a panic could have left half-applied: the slot holds one value.
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<PendingArtifact>> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Read a file the Host chose, refusing anything oversized.
///
/// Checked **twice**: the size the filesystem reports before the read, and the
/// number of bytes actually read. A file that grows between the two cannot get
/// past the bound, and nothing reads a file of unknown size into memory
/// (ADR-0022 decision 16).
pub fn read_bounded(path: &Path) -> HostResult<String> {
    let metadata = std::fs::metadata(path).map_err(|error| unreadable(path, &error))?;

    if metadata.len() > MAX_SUBMISSION_BYTES as u64 {
        return Err(HostError::from(RemoteError::TooLarge {
            limit: MAX_SUBMISSION_BYTES,
            detected: metadata.len(),
        }));
    }

    let bytes = std::fs::read(path).map_err(|error| unreadable(path, &error))?;

    if bytes.len() > MAX_SUBMISSION_BYTES {
        return Err(HostError::from(RemoteError::TooLarge {
            limit: MAX_SUBMISSION_BYTES,
            detected: bytes.len() as u64,
        }));
    }

    String::from_utf8(bytes).map_err(|_| {
        HostError::from(RemoteError::Malformed {
            detail: "the file is not valid UTF-8 text, so it is not a submission".to_owned(),
        })
    })
}

/// Take the first file a drag-drop event offered.
///
/// Extracted from `run()` so it can be tested: the Tauri handler is a one-line
/// delegation, and everything decidable about a drop is decided here.
///
/// A drop may carry several paths. The first is taken rather than all of them,
/// because there is one pending slot and one submission per import - bulk import
/// is explicitly out of scope.
pub fn accept_dropped(pending: &PendingImport, paths: &[PathBuf]) -> HostResult<PendingArtifact> {
    let first = paths.first().ok_or_else(|| {
        HostError::new(
            HostErrorKind::Validation {
                field: "submission".to_owned(),
                expected: "one submission file".to_owned(),
                detected: "an empty drop".to_owned(),
            },
            "Nothing was dropped. Drop a single submission file.".to_owned(),
        )
    })?;

    pending.accept_file(first)
}

/// Take a drop and tell the window that something is waiting.
///
/// The Tauri-side entry point, kept to what needs an `AppHandle`: everything
/// decidable about a drop happens in [`accept_dropped`], which is testable
/// without a running application.
///
/// A refused drop - oversized, unreadable, not UTF-8 - is announced too, so the
/// Host is told why rather than left wondering whether the file arrived.
pub fn announce_drop<R: tauri::Runtime>(app: &tauri::AppHandle<R>, paths: &[PathBuf]) {
    let pending = app.state::<PendingImport>();

    let payload = match accept_dropped(&pending, paths) {
        Ok(artifact) => DroppedDto {
            origin: artifact.origin,
            bytes: artifact.raw.len(),
            error: None,
        },
        Err(error) => DroppedDto {
            origin: paths
                .first()
                .and_then(|p| p.file_name())
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            bytes: 0,
            error: Some(error.message),
        },
    };

    if let Err(error) = app.emit(REMOTE_SUBMISSION_PENDING, payload) {
        // The import still works - the Host can paste, or drop again - so this
        // is a diagnostic rather than a failure. Nothing leaves the device.
        eprintln!("[host] announcing a dropped submission: {error}");
    }
}

/// What the window is told when a file is dropped on it.
///
/// Deliberately not the path: the window learns that something arrived and what
/// it was called, and nothing about where it lives on disk.
#[derive(Debug, Clone, serde::Serialize)]
struct DroppedDto {
    origin: String,
    bytes: usize,
    /// Present when the drop was refused, so the Host is told why.
    error: Option<String>,
}

/// The Host's own machine refused to give up the file.
fn unreadable(path: &Path, error: &std::io::Error) -> HostError {
    eprintln!("[host] reading {}: {error}", path.display());
    HostError::new(
        HostErrorKind::Persistence,
        format!(
            "The submission file at {} could not be read. \
             Check that it is still there and try again.",
            path.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).expect("write");
        path
    }

    #[test]
    fn a_dropped_file_becomes_the_pending_artefact() {
        let dir = TempDir::new().expect("temp dir");
        let path = write(&dir, "submission.json", b"{\"schema_version\":1}");

        let pending = PendingImport::new();
        let accepted = accept_dropped(&pending, &[path]).expect("accepted");

        assert_eq!(accepted.raw, "{\"schema_version\":1}");
        assert_eq!(accepted.origin, "submission.json");
        assert_eq!(pending.peek(), Some(accepted));
    }

    #[test]
    fn an_empty_drop_is_refused() {
        let pending = PendingImport::new();
        assert!(accept_dropped(&pending, &[]).is_err());
        assert!(pending.peek().is_none());
    }

    #[test]
    fn only_the_first_dropped_file_is_taken() {
        // One slot, one submission. Bulk import is out of scope.
        let dir = TempDir::new().expect("temp dir");
        let first = write(&dir, "first.json", b"first");
        let second = write(&dir, "second.json", b"second");

        let pending = PendingImport::new();
        accept_dropped(&pending, &[first, second]).expect("accepted");

        assert_eq!(pending.peek().expect("pending").raw, "first");
    }

    #[test]
    fn an_oversized_file_is_refused_and_leaves_the_slot_alone() {
        let dir = TempDir::new().expect("temp dir");
        let path = write(&dir, "big.json", &vec![b'x'; MAX_SUBMISSION_BYTES + 1]);

        let pending = PendingImport::new();
        let error = pending.accept_file(&path).unwrap_err();

        assert_eq!(error.category, crate::error::ErrorCategory::Validation);
        assert!(error.message.contains("too large"), "{}", error.message);
        assert!(pending.peek().is_none());
    }

    #[test]
    fn a_file_exactly_at_the_limit_is_accepted() {
        let dir = TempDir::new().expect("temp dir");
        let path = write(&dir, "edge.json", &vec![b'x'; MAX_SUBMISSION_BYTES]);

        let pending = PendingImport::new();
        assert!(pending.accept_file(&path).is_ok());
    }

    #[test]
    fn oversized_pasted_text_is_refused_by_the_same_bound() {
        let pending = PendingImport::new();
        let error = pending
            .accept_text("x".repeat(MAX_SUBMISSION_BYTES + 1))
            .unwrap_err();

        assert!(error.message.contains("too large"), "{}", error.message);
        assert!(pending.peek().is_none());
    }

    #[test]
    fn a_file_that_is_not_utf8_is_not_a_submission() {
        let dir = TempDir::new().expect("temp dir");
        let path = write(&dir, "binary.json", &[0xff, 0xfe, 0x00, 0x01]);

        let pending = PendingImport::new();
        let error = pending.accept_file(&path).unwrap_err();
        assert_eq!(error.category, crate::error::ErrorCategory::Validation);
    }

    #[test]
    fn a_missing_file_is_the_hosts_own_problem() {
        let dir = TempDir::new().expect("temp dir");
        let pending = PendingImport::new();

        let error = pending
            .accept_file(&dir.path().join("absent.json"))
            .unwrap_err();
        assert_eq!(error.category, crate::error::ErrorCategory::Unexpected);
    }

    #[test]
    fn the_extension_is_only_a_hint() {
        // A `.json` name does not make malformed JSON valid, and a `.txt` name
        // does not make valid JSON invalid. Parsing decides, later.
        let dir = TempDir::new().expect("temp dir");
        let pending = PendingImport::new();

        for name in ["submission.txt", "submission.json", "submission"] {
            let path = write(&dir, name, b"{\"a\":1}");
            assert_eq!(
                pending.accept_file(&path).expect("accepted").raw,
                "{\"a\":1}"
            );
        }
    }

    #[test]
    fn requiring_an_absent_artefact_says_what_to_do() {
        let pending = PendingImport::new();
        let error = pending.require().unwrap_err();
        assert!(
            error.message.contains("Drop a submission file"),
            "{}",
            error.message
        );
    }

    #[test]
    fn clearing_forgets_what_was_waiting() {
        let pending = PendingImport::new();
        pending.accept_text("{}".to_owned()).expect("accepted");
        assert!(pending.peek().is_some());

        pending.clear();
        assert!(pending.peek().is_none());
    }

    #[test]
    fn a_second_artefact_replaces_the_first() {
        let pending = PendingImport::new();
        pending.accept_text("first".to_owned()).expect("accepted");
        pending.accept_text("second".to_owned()).expect("accepted");

        assert_eq!(pending.peek().expect("pending").raw, "second");
    }
}
