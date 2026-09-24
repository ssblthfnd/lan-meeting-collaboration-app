//! The pure input to every renderer, and the three renderers themselves.
//!
//! Everything here is a plain data structure and a pure function: no
//! database access, no filesystem access, no Tauri API, no authorization
//! decision. The caller (`src-tauri`) reads `MeetingDetail`,
//! `ParticipantSummary` and `NoteDetail` from `HostQueries`, projects them
//! into [`ExportDocument`], and hands it here. Nothing in this module knows
//! those types exist.

use crate::markdown_ast::parse_markdown;
use crate::plain_text::render_plain_text;

/// Meeting metadata, already formatted as display strings by the caller.
///
/// Every field PRD section 20/25.4 asks an export to state is here, and
/// nothing else - no internal id, no configuration history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportMeeting {
    pub title: String,
    pub topic: Option<String>,
    pub date: String,
    pub start_time: String,
    pub end_time: String,
    pub timezone: String,
    pub location: Option<String>,
    pub description: Option<String>,
    /// `"OPEN"` or `"LOCKED"` - a `DRAFT` meeting is refused before a
    /// document is ever assembled (E-2/E-3).
    pub status: String,
    pub locked_at: Option<String>,
}

/// A participant's own note, latest version only (E-4 of the frozen design:
/// no history, no remote-submission ledger).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportNote {
    pub content: String,
    pub version: i64,
    /// `"HOST"`, `"PARTICIPANT"` or `"REMOTE_IMPORT"`.
    pub last_author_type: String,
}

/// One roster row.
///
/// Deliberately narrow: name, department, position, meeting role - exactly
/// PRD section 7's "participant memiliki" fields (E-8). No participant id, no
/// claim status, no session state, no join token ever reaches this struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportParticipant {
    pub name: String,
    pub department: Option<String>,
    pub position: Option<String>,
    pub meeting_role: Option<String>,
    pub note: Option<ExportNote>,
}

/// The bounded meeting-lifecycle timeline (E-7): created, opened, locked.
/// `opened_at` is unconditionally present for any meeting a document is ever
/// assembled for, since `OPEN`/`LOCKED` both imply the meeting passed through
/// `Domain::open_meeting` (E-4 of the corrected design).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportTimeline {
    pub created_at: String,
    pub opened_at: String,
    pub locked_at: Option<String>,
}

/// Everything a renderer needs, and nothing it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDocument {
    pub meeting: ExportMeeting,
    /// Already ordered `name ASC, id ASC` by the caller (the same order
    /// `HostQueries::participants` already uses everywhere else).
    pub participants: Vec<ExportParticipant>,
    pub timeline: ExportTimeline,
}

const NO_NOTE_MARKDOWN: &str = "_No note written._";
const NO_NOTE_PLAIN: &str = "No note written.";

/// Render the Markdown export.
///
/// Note content is emitted **verbatim** - never re-parsed, never
/// re-serialised. A note's own heading, if it has one, is not re-levelled to
/// nest under the participant's section heading; it keeps whatever level the
/// author wrote, even a document-root-level `#`. This is intentional
/// (frozen Markdown Hierarchy Decision): re-levelling would mean rewriting
/// canonical note content for cosmetic purposes, which this renderer never
/// does.
#[must_use]
pub fn render_markdown(document: &ExportDocument) -> String {
    let mut sections: Vec<String> = Vec::new();

    sections.push(format!("# {}", document.meeting.title));
    sections.push(metadata_block(&document.meeting));
    sections.push(timeline_block_markdown(
        &document.timeline,
        &document.meeting,
    ));
    sections.push(participants_block_markdown(&document.participants));

    join_sections(&sections)
}

fn metadata_block(meeting: &ExportMeeting) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "**Date:** {} · {}–{} ({})",
        meeting.date, meeting.start_time, meeting.end_time, meeting.timezone
    ));
    if let Some(location) = &meeting.location {
        lines.push(format!("**Location:** {location}"));
    }
    if let Some(topic) = &meeting.topic {
        lines.push(format!("**Topic:** {topic}"));
    }
    if let Some(description) = &meeting.description {
        lines.push(format!("**Description:** {description}"));
    }
    lines.push(format!("**Status:** {}", meeting.status));
    if let Some(locked_at) = &meeting.locked_at {
        lines.push(format!("**Locked:** {locked_at}"));
    }
    lines.join("\n")
}

fn timeline_block_markdown(timeline: &ExportTimeline, meeting: &ExportMeeting) -> String {
    let mut lines = vec![
        "## Meeting Timeline".to_owned(),
        String::new(),
        format!("- Created: {}", timeline.created_at),
        format!("- Opened: {}", timeline.opened_at),
    ];
    if meeting.locked_at.is_some() {
        if let Some(locked_at) = &timeline.locked_at {
            lines.push(format!("- Locked: {locked_at}"));
        }
    }
    lines.join("\n")
}

fn participants_block_markdown(participants: &[ExportParticipant]) -> String {
    let mut sections = vec!["## Participants".to_owned()];
    for participant in participants {
        sections.push(participant_section_markdown(participant));
    }
    sections.join("\n\n")
}

fn participant_section_markdown(participant: &ExportParticipant) -> String {
    let heading = format!(
        "### {}{}",
        participant.name,
        participant_detail_suffix(participant)
    );
    let body = match &participant.note {
        Some(note) => format!(
            "{}\n\n*(Version {}, last edited by {})*",
            note.content, note.version, note.last_author_type
        ),
        None => NO_NOTE_MARKDOWN.to_owned(),
    };
    format!("{heading}\n\n{body}")
}

fn participant_detail_suffix(participant: &ExportParticipant) -> String {
    let details: Vec<&str> = [
        participant.department.as_deref(),
        participant.position.as_deref(),
        participant.meeting_role.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();

    if details.is_empty() {
        String::new()
    } else {
        format!(" ({})", details.join(" · "))
    }
}

/// Render the TXT export: the same section structure as Markdown, with every
/// heading and note body passed through the bounded parser/plain-text
/// renderer (E-4) instead of being emitted as raw Markdown syntax.
#[must_use]
pub fn render_txt(document: &ExportDocument) -> String {
    render_plain_document(document, None)
}

/// Render the AI Context export: the TXT structure, preceded by a fixed,
/// non-factual preamble, with no heading recognition of any kind (frozen
/// corrected E-1). Every author-written heading survives structurally,
/// because nothing here inspects a heading's text - only its block type.
#[must_use]
pub fn render_ai_context(document: &ExportDocument) -> String {
    const PREAMBLE: &str = "The following is a structured export of a meeting record. It has been formatted only; nothing has been summarized, inferred, or added.";
    render_plain_document(document, Some(PREAMBLE))
}

fn render_plain_document(document: &ExportDocument, preamble: Option<&str>) -> String {
    let mut sections: Vec<String> = Vec::new();

    if let Some(preamble) = preamble {
        sections.push(preamble.to_owned());
    }

    sections.push(plain_text_of_markdown(&document.meeting.title));
    sections.push(metadata_block_plain(&document.meeting));
    sections.push(timeline_block_plain(&document.timeline, &document.meeting));
    sections.push(participants_block_plain(&document.participants));

    join_sections(&sections)
}

fn metadata_block_plain(meeting: &ExportMeeting) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Date: {} {}-{} ({})",
        meeting.date, meeting.start_time, meeting.end_time, meeting.timezone
    ));
    if let Some(location) = &meeting.location {
        lines.push(format!("Location: {location}"));
    }
    if let Some(topic) = &meeting.topic {
        lines.push(format!("Topic: {topic}"));
    }
    if let Some(description) = &meeting.description {
        lines.push(format!("Description: {description}"));
    }
    lines.push(format!("Status: {}", meeting.status));
    if let Some(locked_at) = &meeting.locked_at {
        lines.push(format!("Locked: {locked_at}"));
    }
    lines.join("\n")
}

fn timeline_block_plain(timeline: &ExportTimeline, meeting: &ExportMeeting) -> String {
    let mut lines = vec![
        "Meeting Timeline".to_owned(),
        format!("Created: {}", timeline.created_at),
        format!("Opened: {}", timeline.opened_at),
    ];
    if meeting.locked_at.is_some() {
        if let Some(locked_at) = &timeline.locked_at {
            lines.push(format!("Locked: {locked_at}"));
        }
    }
    lines.join("\n")
}

fn participants_block_plain(participants: &[ExportParticipant]) -> String {
    let mut sections = vec!["Participants".to_owned()];
    for participant in participants {
        sections.push(participant_section_plain(participant));
    }
    sections.join("\n\n")
}

fn participant_section_plain(participant: &ExportParticipant) -> String {
    let mut heading = participant.name.clone();
    let suffix = participant_detail_suffix(participant);
    if !suffix.is_empty() {
        heading.push_str(&suffix);
    }

    let body = match &participant.note {
        Some(note) => format!(
            "{}\n\n(Version {}, last edited by {})",
            plain_text_of_markdown(&note.content),
            note.version,
            note.last_author_type
        ),
        None => NO_NOTE_PLAIN.to_owned(),
    };

    format!("{heading}\n\n{body}")
}

/// Parse and flatten stored note Markdown to the plain-text contract (E-4).
fn plain_text_of_markdown(markdown: &str) -> String {
    render_plain_text(&parse_markdown(markdown))
}

/// Join top-level sections with exactly one blank line between them, and end
/// the file with a single trailing newline.
fn join_sections(sections: &[String]) -> String {
    let mut joined = sections.join("\n\n");
    joined.push('\n');
    joined
}
