//! **A committer the door stamps, and an author that is verified or claimed** (ledger #2827).
//!
//! Until 0.15.0 `author=` was free text any caller could send, and no write recorded the
//! principal that made it. Now every item and comment records `ledger:committer`, the
//! principal the capability carried, and an author is accepted only when it is the committer,
//! a principal the committer acts for (`prov:actedOnBehalfOf` in the host's participants
//! graph), or, for an import principal (`ledger:Importer`), a foreign actor. Only an import
//! principal may claim authored dates. One test per path, each read back on every face: the
//! plain detail, the JSON document and the Turtle graph.

mod common;

use std::sync::Arc;

use common::*;
use ikigai_core::{Capability, Error, Fallback, Kernel, Space, Verb};
use ikigai_ledger::SpaceConfig;
use ikigai_store::DurableStore;

const LEDGER: &str = "acme";
const PARTICIPANTS: &str = "urn:collab:test:participants";
const ADA: &str = "urn:collab:participant:ada";
const ADA_AGENT: &str = "urn:collab:participant:ada-agent";
const BOB: &str = "urn:collab:participant:bob";
const IMPORTER: &str = "urn:iki:gonk:client:importer";
const FOREIGN: &str = "urn:example:github:user:octocat";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

/// A kernel whose ledger reads enrollment from [`PARTICIPANTS`], and that graph: Ada's agent
/// acts for Ada, and the importer is an import principal. Bob is enrolled for nobody.
fn kernel() -> Kernel {
    let ledger = ikigai_ledger::space_with(SpaceConfig::default().participants(PARTICIPANTS));
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ledger) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    sink(
        &kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!(
                "INSERT DATA {{ GRAPH <{PARTICIPANTS}> {{ \
                 <{ADA_AGENT}> <http://www.w3.org/ns/prov#actedOnBehalfOf> <{ADA}> . \
                 <{IMPORTER}> a <https://ikigai-rs.dev/ns/ledger#Importer> . }} }}"
            ),
        )],
    );
    kernel
}

/// A writer's grant, as a door mints it for one principal: this ledger, and the
/// participants graph's read grant, which enrollment is read with.
fn writer(principal: &str) -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:write:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
        format!("urn:cap:store:read:graph:{PARTICIPANTS}"),
    ])
    .with_principal(principal)
    .unwrap()
}

fn file(kernel: &Kernel, who: &Capability, args: &[(&str, &str)]) -> Result<String, Error> {
    let mut all = vec![("content", "Fix the widget")];
    all.extend_from_slice(args);
    try_as(kernel, who, Verb::Sink, &at("append"), &all)
}

fn comment(kernel: &Kernel, who: &Capability, args: &[(&str, &str)]) -> Result<String, Error> {
    let mut all = vec![("item", "1"), ("content", "a note")];
    all.extend_from_slice(args);
    try_as(kernel, who, Verb::Sink, &at("comment"), &all)
}

/// The item's three faces: plain detail, JSON, Turtle.
fn faces(kernel: &Kernel) -> (String, serde_json::Value, String) {
    let plain = source(kernel, &at("item:1"), &[]);
    let json = source(kernel, &at("item:1"), &[("as", "application/json")]);
    let turtle = source(kernel, &at("item:1"), &[("as", "text/turtle")]);
    (plain, serde_json::from_str(&json).expect("JSON"), turtle)
}

fn has(text: &str, wanted: &str) {
    assert!(text.contains(wanted), "{wanted:?} is not in:\n{text}");
}

fn lacks(text: &str, unwanted: &str) {
    assert!(!text.contains(unwanted), "{unwanted:?} is in:\n{text}");
}

fn denied(result: Result<String, Error>, words: &[&str]) {
    match result {
        Err(Error::Denied(message)) => {
            for word in words {
                has(&message, word);
            }
        }
        other => panic!("expected a denial naming {words:?}, got {other:?}"),
    }
}

fn nothing_filed(kernel: &Kernel) {
    let count = source(kernel, &at("count"), &[("status", "all")]);
    assert_eq!(count.trim(), "0", "a refused write filed something");
}

// ------------------------------------------------------------------------ as yourself

#[test]
fn writing_as_yourself_is_a_verified_author_on_every_face() {
    let kernel = kernel();
    file(&kernel, &writer(ADA), &[]).unwrap();
    // Naming yourself is the same as naming no one.
    comment(&kernel, &writer(ADA), &[("author", ADA)]).unwrap();
    let (plain, json, turtle) = faces(&kernel);

    has(&plain, &format!("  author:   {ADA} (verified)\n"));
    has(&plain, &format!("  committer: {ADA} at "));
    has(&plain, &format!("{ADA} (verified)\n    a note"));
    lacks(&plain, "(claimed");

    let who = &json["item"]["attribution"];
    assert_eq!(who["author"], ADA);
    assert_eq!(who["author_status"], "verified");
    assert_eq!(who["author_source"], "committer");
    assert_eq!(who["committer"], ADA);
    assert_eq!(who["claimed_dates"], serde_json::json!([]));
    assert_eq!(json["item"]["author"], ADA);
    let note = &json["item"]["comments"][0]["attribution"];
    assert_eq!(note["author_status"], "verified");
    assert_eq!(note["committer"], ADA);

    has(&turtle, &format!("ledger:author <{ADA}>"));
    has(&turtle, "ledger:authorSource \"committer\"");
    has(&turtle, &format!("ledger:committer <{ADA}>"));
    has(&turtle, "ledger:committedAt ");
}

// ------------------------------------------------------------- as an enrolled delegate

#[test]
fn an_enrolled_delegate_records_a_claimed_author_and_itself_as_committer() {
    let kernel = kernel();
    file(&kernel, &writer(ADA_AGENT), &[("author", ADA)]).unwrap();
    comment(&kernel, &writer(ADA_AGENT), &[("author", ADA)]).unwrap();
    let (plain, json, turtle) = faces(&kernel);

    has(&plain, &format!(" by {ADA}\n  updated:  "));
    has(
        &plain,
        &format!("  author:   {ADA} (claimed by a delegate)\n"),
    );
    has(&plain, &format!("  committer: {ADA_AGENT} at "));
    has(
        &plain,
        &format!("{ADA} (claimed by a delegate), committed by {ADA_AGENT}\n"),
    );

    let who = &json["item"]["attribution"];
    assert_eq!(who["author"], ADA);
    assert_eq!(who["author_status"], "claimed");
    assert_eq!(who["author_source"], "delegate");
    assert_eq!(who["committer"], ADA_AGENT);
    let note = &json["item"]["comments"][0]["attribution"];
    assert_eq!(note["author_source"], "delegate");
    assert_eq!(note["committer"], ADA_AGENT);

    has(&turtle, &format!("ledger:author <{ADA}>"));
    has(&turtle, "ledger:authorSource \"delegate\"");
    has(&turtle, &format!("ledger:committer <{ADA_AGENT}>"));
}

/// The write answer is a face too: the comment it returns says who, as the item does.
#[test]
fn a_delegates_comment_answer_carries_its_attribution() {
    let kernel = kernel();
    file(&kernel, &writer(ADA_AGENT), &[]).unwrap();
    let answer = comment(
        &kernel,
        &writer(ADA_AGENT),
        &[("author", ADA), ("as", "application/json")],
    )
    .unwrap();
    let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
    let who = &answer["comment"]["attribution"];
    assert_eq!(who["author"], ADA, "{answer}");
    assert_eq!(who["author_status"], "claimed");
    assert_eq!(who["committer"], ADA_AGENT);
}

// ------------------------------------------- as an import principal, with authored dates

#[test]
fn an_import_principal_claims_a_foreign_author_and_authored_dates() {
    let kernel = kernel();
    file(
        &kernel,
        &writer(IMPORTER),
        &[
            ("author", FOREIGN),
            ("created", "2020-01-02T03:04:05Z"),
            ("modified", "2020-02-03T04:05:06Z"),
        ],
    )
    .unwrap();
    let (plain, json, turtle) = faces(&kernel);

    has(
        &plain,
        &format!("  filed:    2020-01-02T03:04:05.000Z (claimed) by {FOREIGN}\n"),
    );
    has(&plain, "  updated:  2020-02-03T04:05:06.000Z (claimed)\n");
    has(
        &plain,
        &format!("  author:   {FOREIGN} (claimed by an importer)\n"),
    );
    has(&plain, &format!("  committer: {IMPORTER} at 2026-09-15T"));

    let item = &json["item"];
    // The stored dates are the claimed ones, so ranking tie-breaks as the source did.
    assert_eq!(item["created"], "2020-01-02T03:04:05.000Z");
    assert_eq!(item["modified"], "2020-02-03T04:05:06.000Z");
    let who = &item["attribution"];
    assert_eq!(who["author"], FOREIGN);
    assert_eq!(who["author_status"], "claimed");
    assert_eq!(who["author_source"], "import");
    assert_eq!(who["committer"], IMPORTER);
    assert_eq!(
        who["claimed_dates"],
        serde_json::json!(["created", "modified"])
    );

    has(&turtle, &format!("ledger:author <{FOREIGN}>"));
    has(&turtle, "ledger:authorSource \"import\"");
    has(&turtle, "ledger:createdSource \"import\"");
    has(&turtle, "ledger:modifiedSource \"import\"");
    has(&turtle, &format!("ledger:committer <{IMPORTER}>"));

    // A comment with an authored `created`. Since 0.18.0 a claimed date moves the item's
    // `modified` only forward (ledger #2928), so this earlier one leaves the claim standing.
    comment(
        &kernel,
        &writer(IMPORTER),
        &[("author", FOREIGN), ("created", "2020-01-05T00:00:00Z")],
    )
    .unwrap();
    let (plain, json, turtle) = faces(&kernel);
    has(
        &plain,
        &format!(
            "  2020-01-05T00:00:00.000Z (claimed) {FOREIGN} (claimed by an importer), \
             committed by {IMPORTER}\n"
        ),
    );
    has(&plain, "  updated:  2020-02-03T04:05:06.000Z (claimed)\n");
    let note = &json["item"]["comments"][0];
    assert_eq!(note["time"], "2020-01-05T00:00:00.000Z");
    assert_eq!(
        note["attribution"]["claimed_dates"],
        serde_json::json!(["created"])
    );
    assert_eq!(
        json["item"]["attribution"]["claimed_dates"],
        serde_json::json!(["created", "modified"])
    );
    has(&turtle, "ledger:createdSource \"import\"");
}

/// A claimed `modified` is a claim only until the ledger next stamps the item.
#[test]
fn a_later_write_ends_a_claimed_modified() {
    let kernel = kernel();
    file(
        &kernel,
        &writer(IMPORTER),
        &[("modified", "2020-02-03T04:05:06Z")],
    )
    .unwrap();
    sink(
        &kernel,
        &at("label"),
        &[("item", "1"), ("content", "triage")],
    );
    let (plain, json, turtle) = faces(&kernel);
    lacks(&plain, "(claimed)");
    assert_eq!(
        json["item"]["attribution"]["claimed_dates"],
        serde_json::json!([])
    );
    lacks(&turtle, "modifiedSource");
}

// ----------------------------------------------------------------------- refused author

#[test]
fn an_author_the_committer_does_not_act_for_is_refused_with_the_remedy() {
    let kernel = kernel();
    // Bob is enrolled for nobody, and is no importer.
    denied(
        file(&kernel, &writer(BOB), &[("author", ADA)]),
        &[
            &format!("`author={ADA}` refused for committer {BOB}"),
            "prov:actedOnBehalfOf",
            "To write anyway, send no author",
        ],
    );
    // A plain name is not a principal, whoever sends it.
    denied(
        file(&kernel, &writer(BOB), &[("author", "brian")]),
        &["not a name", "send the IRI"],
    );
    nothing_filed(&kernel);
    // And on a comment: nothing written.
    file(&kernel, &writer(BOB), &[]).unwrap();
    denied(
        comment(&kernel, &writer(BOB), &[("author", ADA)]),
        &["refused"],
    );
    let json = faces(&kernel).1;
    assert_eq!(json["item"]["comments"], serde_json::json!([]));
}

#[test]
fn a_host_without_a_participants_graph_admits_only_the_committer() {
    let kernel = common::kernel();
    let mine = Capability::scoped(vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:write:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
    ])
    .with_principal(ADA_AGENT)
    .unwrap();
    denied(
        file(&kernel, &mine, &[("author", ADA)]),
        &["this host names no participants graph"],
    );
    file(&kernel, &mine, &[]).unwrap();
    has(
        &source(&kernel, &at("item:1"), &[]),
        &format!("author:   {ADA_AGENT} (verified)"),
    );
}

// ------------------------------------------------------------------------- refused date

#[test]
fn a_date_from_anyone_but_an_import_principal_is_refused_and_says_why() {
    let kernel = kernel();
    // Not an importer, though enrolled as a delegate.
    denied(
        file(
            &kernel,
            &writer(ADA_AGENT),
            &[("created", "2020-01-01T00:00:00Z")],
        ),
        &[
            "only an import or sync principal may claim an authored date",
            &format!("{ADA_AGENT} is not an import or sync principal"),
            "Send it without `created`",
        ],
    );
    // Root carries no principal at all.
    match try_verb(
        &kernel,
        Verb::Sink,
        &at("append"),
        &[("content", "x"), ("modified", "2020-01-01T00:00:00Z")],
    ) {
        Err(Error::Denied(message)) => has(&message, "carries no principal"),
        other => panic!("root's date was not refused: {other:?}"),
    }
    nothing_filed(&kernel);
    file(&kernel, &writer(ADA), &[]).unwrap();
    denied(
        comment(
            &kernel,
            &writer(ADA),
            &[("created", "2020-01-01T00:00:00Z")],
        ),
        &["only an import or sync principal"],
    );
}

#[test]
fn an_importers_date_must_be_a_past_date_time() {
    let kernel = kernel();
    for (date, why) in [
        ("2020-01-01", "is not an xsd:dateTime"),
        ("2999-01-01T00:00:00Z", "cannot be later than its import"),
    ] {
        match file(&kernel, &writer(IMPORTER), &[("created", date)]) {
            Err(Error::InvalidArgument { name, detail }) => {
                assert_eq!(name, "created");
                has(&detail, why);
            }
            other => panic!("{date} was not refused: {other:?}"),
        }
    }
    nothing_filed(&kernel);
}

// --------------------------------------------------------------- no principal: unchanged

#[test]
fn a_write_without_a_principal_keeps_free_text_and_reads_it_as_claimed() {
    let kernel = kernel();
    sink(
        &kernel,
        &at("append"),
        &[("content", "Fix the widget"), ("author", "brian")],
    );
    let (plain, json, turtle) = faces(&kernel);
    has(&plain, "  author:   brian (claimed, free text)\n");
    lacks(&plain, "committer:");
    assert_eq!(json["item"]["attribution"]["author_status"], "claimed");
    assert_eq!(
        json["item"]["attribution"]["author_source"],
        serde_json::Value::Null
    );
    has(&turtle, "ledger:author \"brian\"");
    lacks(&turtle, "ledger:committer");
}

// ------------------------- an importer's claimed dates and the item's `modified` (ledger #2928)

/// The item's JSON `modified` and its claimed dates.
fn modified(kernel: &Kernel, number: &str) -> (String, serde_json::Value) {
    let json: serde_json::Value = serde_json::from_str(&source(
        kernel,
        &at(&format!("item:{number}")),
        &[("as", "application/json")],
    ))
    .expect("JSON");
    (
        json["item"]["modified"].as_str().unwrap().to_string(),
        json["item"]["attribution"]["claimed_dates"].clone(),
    )
}

/// ★ The reproduction (ledger #2928): an importer files with the source's `modified`, then
/// replays the item's past (a comment dated at the source, a close dated at the source). Until
/// 0.18.0 each of those writes stamped the kernel's time, so `modified` ended at the import and
/// `priority-recency` ranked the item as just updated. A claimed date moves `modified` only to
/// the later of the two, so a past date leaves it where the filing put it.
#[test]
fn an_importers_past_comment_and_close_leave_the_claimed_modified() {
    let kernel = kernel();
    let importer = writer(IMPORTER);
    file(
        &kernel,
        &importer,
        &[
            ("created", "2020-01-02T03:04:05Z"),
            ("modified", "2020-02-03T04:05:06Z"),
        ],
    )
    .unwrap();
    comment(&kernel, &importer, &[("created", "2020-01-05T00:00:00Z")]).unwrap();
    assert_eq!(
        modified(&kernel, "1"),
        (
            "2020-02-03T04:05:06.000Z".to_string(),
            serde_json::json!(["created", "modified"])
        ),
        "a past comment leaves the claimed modified"
    );
    try_as(
        &kernel,
        &importer,
        Verb::Sink,
        &at("close"),
        &[("item", "1"), ("closed-at", "2020-01-20T00:00:00Z")],
    )
    .unwrap();
    let (plain, json, turtle) = faces(&kernel);
    assert_eq!(json["item"]["modified"], "2020-02-03T04:05:06.000Z");
    has(&plain, "  updated:  2020-02-03T04:05:06.000Z (claimed)\n");
    has(&turtle, "ledger:modifiedSource \"import\"");
}

/// A claimed date after the current `modified` moves it there, and the item says it is claimed.
#[test]
fn a_claimed_date_after_the_modified_moves_it_to_that_date() {
    let kernel = kernel();
    let importer = writer(IMPORTER);
    file(
        &kernel,
        &importer,
        &[
            ("created", "2020-01-01T00:00:00Z"),
            ("modified", "2020-02-03T04:05:06Z"),
        ],
    )
    .unwrap();
    comment(&kernel, &importer, &[("created", "2020-03-01T00:00:00Z")]).unwrap();
    let (when, claimed) = modified(&kernel, "1");
    assert_eq!(when, "2020-03-01T00:00:00.000Z");
    assert_eq!(claimed, serde_json::json!(["created", "modified"]));
    try_as(
        &kernel,
        &importer,
        Verb::Sink,
        &at("close"),
        &[("item", "1"), ("closed-at", "2020-04-01T00:00:00Z")],
    )
    .unwrap();
    assert_eq!(modified(&kernel, "1").0, "2020-04-01T00:00:00.000Z");
}

/// A write without a claimed date stamps the kernel's time, the importer's and anyone else's.
#[test]
fn a_write_without_a_claimed_date_still_stamps_now() {
    let kernel = kernel();
    file(
        &kernel,
        &writer(IMPORTER),
        &[("modified", "2020-02-03T04:05:06Z")],
    )
    .unwrap();
    comment(&kernel, &writer(IMPORTER), &[]).unwrap();
    let (when, claimed) = modified(&kernel, "1");
    assert!(
        when.starts_with("2026-"),
        "the importer's undated write: {when}"
    );
    assert_eq!(claimed, serde_json::json!([]));

    file(
        &kernel,
        &writer(IMPORTER),
        &[("modified", "2020-02-03T04:05:06Z")],
    )
    .unwrap();
    try_as(
        &kernel,
        &writer(ADA),
        Verb::Sink,
        &at("comment"),
        &[("item", "2"), ("content", "an ordinary note")],
    )
    .unwrap();
    let (when, claimed) = modified(&kernel, "2");
    assert!(
        when.starts_with("2026-"),
        "an ordinary principal's write: {when}"
    );
    assert_eq!(claimed, serde_json::json!([]));
}

/// ★ The reproduction (ledger #2932): a claimed date between the filing's commit and now. The
/// importer files, then replays a comment the source dated AFTER that commit (an import that
/// runs while the source is still being written to). `modified` moves to that date, which is
/// right, and until 0.18.0 the item then read it as NOT claimed: the read took a claimed
/// `modified` to hold only while it was earlier than `committedAt`. An undated write after it
/// still ends the claim (#2928's rule).
#[test]
fn a_claimed_modified_after_the_filing_commit_reads_as_claimed() {
    let kernel = kernel();
    let importer = writer(IMPORTER);
    file(
        &kernel,
        &importer,
        &[
            ("created", "2020-01-02T03:04:05Z"),
            ("modified", "2020-02-03T04:05:06Z"),
        ],
    )
    .unwrap();
    let (_, json, _) = faces(&kernel);
    let committed = json["item"]["attribution"]["committed"]
        .as_str()
        .expect("the filing's commit")
        .to_string();
    // The test clock ticks whole seconds, so half a second after the commit is after it and
    // before the comment's own write.
    assert!(committed.ends_with(".000Z"), "{committed}");
    let between = committed.replace(".000Z", ".500Z");
    comment(&kernel, &importer, &[("created", &between)]).unwrap();
    let (plain, json, turtle) = faces(&kernel);
    assert_eq!(json["item"]["modified"], serde_json::json!(between));
    assert_eq!(
        json["item"]["attribution"]["claimed_dates"],
        serde_json::json!(["created", "modified"]),
        "a claimed date after the filing's commit is still claimed"
    );
    has(&plain, &format!("  updated:  {between} (claimed)\n"));
    has(&turtle, "ledger:modifiedSource \"import\"");

    // An undated write stamps now, and the claim ends.
    comment(&kernel, &importer, &[]).unwrap();
    let (plain, json, turtle) = faces(&kernel);
    assert_ne!(json["item"]["modified"], serde_json::json!(between));
    assert_eq!(
        json["item"]["attribution"]["claimed_dates"],
        serde_json::json!(["created"])
    );
    let updated = plain
        .lines()
        .find(|line| line.starts_with("  updated:"))
        .expect("an updated line");
    lacks(updated, "(claimed)");
    lacks(&turtle, "modifiedSource");
}

/// `next` under `priority-recency` ranks an imported set as the source did: the item updated
/// later at the source first, however its past was replayed.
#[test]
fn next_ranks_an_imported_set_as_the_source_did() {
    let kernel = kernel();
    let importer = writer(IMPORTER);
    for (title, when) in [
        ("Older at the source", "2020-01-01T00:00:00Z"),
        ("Newer at the source", "2020-06-01T00:00:00Z"),
    ] {
        try_as(
            &kernel,
            &importer,
            Verb::Sink,
            &at("append"),
            &[("content", title), ("modified", when)],
        )
        .unwrap();
    }
    // The older item's past, replayed after both were filed.
    comment(&kernel, &importer, &[("created", "2019-12-31T00:00:00Z")]).unwrap();
    let next = source(&kernel, &at("next"), &[]);
    let newer = next.find("Newer at the source").expect(&next);
    let older = next.find("Older at the source").expect(&next);
    assert!(newer < older, "the source's order: {next}");
}
