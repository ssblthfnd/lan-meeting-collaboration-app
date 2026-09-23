//! Generation against the **real** embedded template.
//!
//! The unit tests in `src/generate.rs` use a stand-in shell so that the
//! security-critical properties - escaping, island bounds, the artefact guard -
//! are asserted whether or not anyone has run `npm run build:remote`. These run
//! against the bundle a Host would actually ship.
//!
//! `build.rs` creates an empty `apps/remote-form/dist` on a fresh clone so the
//! crate always compiles, which makes "was the form built?" a separate question.
//! When it has not been, these skip loudly rather than failing: `cargo test`
//! alone on a clean checkout should not report a missing npm build as a code
//! defect. `npm run check` builds the form first, so the real assertions run in
//! the sequence that gates a commit.

use app_remote::{generate, is_bundled, verify_artifact, RemoteFormContext, SCHEMA_VERSION};

use app_core::id::{MeetingId, ParticipantId, SubmissionId};

fn context(existing_content: &str) -> RemoteFormContext {
    RemoteFormContext {
        schema_version: SCHEMA_VERSION,
        submission_id: SubmissionId::new(),
        meeting_id: MeetingId::new(),
        meeting_title: "Weekly Coordination".to_owned(),
        meeting_date: "2026-09-20".to_owned(),
        meeting_timezone: "Asia/Makassar".to_owned(),
        participant_id: ParticipantId::new(),
        participant_name: "Budi Santoso".to_owned(),
        source_version: 3,
        existing_content: existing_content.to_owned(),
        generated_at: "2026-09-23T01:00:00.000Z".to_owned(),
    }
}

/// Skip, loudly, when the npm build has not run.
macro_rules! require_template {
    () => {
        if !is_bundled() {
            eprintln!(
                "[app-remote] skipped: no remote form template is embedded. \
                 Run `npm run build:remote`."
            );
            return;
        }
    };
}

#[test]
fn a_generated_form_is_one_self_contained_file() {
    require_template!();

    let html = generate(&context("## What I noted")).expect("generates");

    // `generate` already ran this; asserting it here is what makes the property
    // visible in the test that people read.
    verify_artifact(&html).expect("self-contained");

    assert!(html.starts_with("<!doctype html"), "{}", &html[..80]);
    assert!(html.contains("</html>"));
}

#[test]
fn the_context_reaches_the_artefact_intact() {
    require_template!();

    let original = context("## What I noted\n\n- First point");
    let html = generate(&original).expect("generates");

    let payload = island_payload(&html);
    let decoded: RemoteFormContext = serde_json::from_str(&payload).expect("readable context");

    assert_eq!(decoded, original);
    assert_eq!(decoded.source_version, 3);
    assert_eq!(decoded.existing_content, "## What I noted\n\n- First point");
}

#[test]
fn an_existing_note_is_prefilled_and_an_absent_one_is_empty() {
    require_template!();

    let written = generate(&context("Something already written")).expect("generates");
    let decoded: RemoteFormContext = serde_json::from_str(&island_payload(&written)).unwrap();
    assert_eq!(decoded.existing_content, "Something already written");

    // A participant with no note gets an empty editor and `source_version` 0.
    let mut fresh = context("");
    fresh.source_version = 0;
    let html = generate(&fresh).expect("generates");
    let decoded: RemoteFormContext = serde_json::from_str(&island_payload(&html)).unwrap();
    assert_eq!(decoded.existing_content, "");
    assert_eq!(decoded.source_version, 0);
}

#[test]
fn the_submission_id_is_the_one_the_host_minted() {
    require_template!();

    let original = context("note");
    let html = generate(&original).expect("generates");
    let decoded: RemoteFormContext = serde_json::from_str(&island_payload(&html)).unwrap();

    assert_eq!(decoded.submission_id, original.submission_id);

    // Two generations for the same participant are two artefacts, and both
    // stay importable (ADR-0021 decision 13).
    let second = generate(&context("note")).expect("generates");
    let other: RemoteFormContext = serde_json::from_str(&island_payload(&second)).unwrap();
    assert_ne!(other.submission_id, original.submission_id);
}

#[test]
fn hostile_content_cannot_escape_the_island_in_the_real_template() {
    require_template!();

    let mut hostile = context(
        "</script><script>alert(1)</script>\n\n\
         <!-- comment --> <img src=x onerror=alert(1)>\n\n\
         separators: \u{2028} \u{2029}",
    );
    hostile.meeting_title = "</SCRIPT ><iframe src=javascript:alert(1)>".to_owned();
    hostile.participant_name = "</script>".to_owned();

    let html = generate(&hostile).expect("generates");
    verify_artifact(&html).expect("still self-contained");

    let payload = island_payload(&html);
    assert!(!payload.contains('<'), "{payload}");
    assert!(!payload.contains('\u{2028}'), "{payload}");
    assert!(!payload.contains('\u{2029}'), "{payload}");

    // And it still decodes to exactly what the Host put in.
    let decoded: RemoteFormContext = serde_json::from_str(&payload).expect("readable");
    assert_eq!(decoded, hostile);
}

#[test]
fn a_generated_form_carries_no_credential_and_no_network_call() {
    require_template!();

    let html = generate(&context("note")).expect("generates");

    for forbidden in [
        "Bearer ",
        "session_token",
        "join_token",
        "token_hash",
        "sessionToken",
        "Authorization",
        "fetch(",
        "XMLHttpRequest",
        "new WebSocket",
        "sendBeacon",
        "EventSource",
        "/join/",
    ] {
        assert!(
            !html.contains(forbidden),
            "the generated form contains `{forbidden}`"
        );
    }
}

#[test]
fn a_note_with_links_is_generated_and_is_not_mistaken_for_a_network_call() {
    require_template!();

    // Links are part of the Markdown subset (ADR-0007). An absolute URL inside
    // the payload is inert text; refusing it would refuse an ordinary note.
    let html = generate(&context(
        "See [the brief](https://example.com/brief)\n\nMail [us](mailto:team@example.com)",
    ))
    .expect("a link in a note is not a network call");

    verify_artifact(&html).expect("self-contained");
    assert!(html.contains("https://example.com/brief"));
}

#[test]
fn the_bundle_runs_after_the_island_exists() {
    require_template!();

    // The single-file build hoists the bundle into `<head>`, ahead of the
    // island it reads. That is fine *because* it is a module script, and a
    // module script without `async` is deferred by specification: it runs after
    // the document has been parsed, so `getElementById` finds the island.
    //
    // The whole form depends on that, and nothing else checks it, so it is
    // asserted here rather than assumed. A build change that made the script
    // classic or `async` would break every generated form silently.
    let html = generate(&context("note")).expect("generates");

    let opening = html
        .match_indices("<script")
        .map(|(at, _)| {
            let end = html[at..].find('>').expect("a tag that closes") + at;
            &html[at..=end]
        })
        .collect::<Vec<_>>();

    let bundle = opening
        .iter()
        .find(|tag| tag.contains("type=\"module\""))
        .expect("the bundle is a module script");

    assert!(
        !bundle.contains("async"),
        "the bundle must stay deferred, so the island exists when it runs: {bundle}"
    );

    // And there is exactly one of each, so "the bundle" and "the island" are
    // unambiguous.
    assert_eq!(
        opening
            .iter()
            .filter(|tag| tag.contains("type=\"module\""))
            .count(),
        1,
        "{opening:?}"
    );
    assert_eq!(
        opening
            .iter()
            .filter(|tag| tag.contains("id=\"submission-context\""))
            .count(),
        1,
        "{opening:?}"
    );
}

/// The island's contents, for a test that wants to read them back.
fn island_payload(html: &str) -> String {
    const OPEN: &str = r#"<script type="application/json" id="submission-context">"#;
    let start = html.find(OPEN).expect("an island") + OPEN.len();
    let end = start + html[start..].find("</script>").expect("a closing tag");
    html[start..end].to_owned()
}
