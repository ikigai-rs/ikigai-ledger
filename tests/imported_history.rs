//! **An importer's history** (ledger #2843): `urn:iki:ledger:{ledger}:history` takes a batch of
//! past-dated claim, release, edit and status events for one item, as Turtle.
//!
//! Until 0.18.0 an import principal could claim an item's, a comment's and a close's dates, but
//! an item's history could not come across. These tests hold the item's acceptance: a batch is
//! recorded with the current state unchanged, a re-send changes nothing, a non-importer is
//! refused, dates outside the rules are refused, and the history faces say imported. And the hub's
//! note: an imported event moves no version a mirror reads, and reorders nothing.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use common::*;
use ikigai_core::{
    ArgRef, Capability, Endpoint, Error, Fallback, Invocation, Iri, Kernel, Representation,
    Request, Resolution, Result, Scope, Space, Verb,
};
use ikigai_ledger::SpaceConfig;
use ikigai_store::DurableStore;

const LEDGER: &str = "acme";
const LIVE: &str = "urn:iki:ledger:graph:acme";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const PARTICIPANTS: &str = "urn:collab:test:participants";
const IMPORTER: &str = "urn:iki:gonk:client:importer";
const DISPATCHER: &str = "urn:iki:gonk:passkey:dispatcher";
const ACTOR: &str = "urn:example:github:user:octocat";
/// What [`TickingClock`] reads first, as the item is filed: every date below is claimed through
/// `created=` at or after this one.
const FILED: &str = "2020-01-01T00:00:00Z";

const PREFIXES: &str = "@prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .\n\
                        @prefix dcterms: <http://purl.org/dc/terms/> .\n\
                        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

fn grants(verbs: &[&str]) -> Vec<String> {
    let mut scopes: Vec<String> = verbs
        .iter()
        .map(|verb| format!("urn:cap:ledger:{verb}:{LEDGER}"))
        .collect();
    scopes.push(graph_read(LEDGER));
    scopes.push(graph_write(LEDGER));
    scopes.push(format!("urn:cap:store:read:graph:{PARTICIPANTS}"));
    scopes
}

/// What a door mints for the principal the participants graph types `ledger:Importer`.
fn importer() -> Capability {
    Capability::scoped(grants(&["read", "write"]))
        .with_principal(IMPORTER)
        .unwrap()
}

/// As strong a grant, for a principal nobody enrolled.
fn dispatcher() -> Capability {
    Capability::scoped(grants(&["read", "write"]))
        .with_principal(DISPATCHER)
        .unwrap()
}

fn enroll(kernel: &Kernel) {
    sink(
        kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!(
                "INSERT DATA {{ GRAPH <{PARTICIPANTS}> {{ \
                 <{IMPORTER}> a <{NS}Importer> . }} }}"
            ),
        )],
    );
}

/// `#1` filed by the importer with the source date [`FILED`], under a kernel that enrolls it.
fn seeded() -> Kernel {
    let ledger = ikigai_ledger::space_with(SpaceConfig::default().participants(PARTICIPANTS));
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ledger) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    enroll(&kernel);
    file(&kernel, "Fix the widget");
    kernel
}

fn file(kernel: &Kernel, title: &str) -> String {
    try_as(
        kernel,
        &importer(),
        Verb::Sink,
        &at("append"),
        &[("content", title), ("created", FILED)],
    )
    .unwrap()
}

/// Send `events` for the item as `who`.
fn send(kernel: &Kernel, who: &Capability, item: &str, events: &str) -> Result<String> {
    let content = format!("{PREFIXES}{events}");
    try_as(
        kernel,
        who,
        Verb::Sink,
        &at("history"),
        &[("item", item), ("content", &content)],
    )
}

fn import(kernel: &Kernel, events: &str) -> String {
    send(kernel, &importer(), "1", events).unwrap_or_else(|e| panic!("{events}: {e}"))
}

fn refusal(kernel: &Kernel, events: &str) -> String {
    let triples = triple_count(kernel);
    let refused = send(kernel, &importer(), "1", events)
        .expect_err("this batch is refused")
        .to_string();
    assert_eq!(
        triple_count(kernel),
        triples,
        "a refused batch writes nothing"
    );
    assert!(refused.contains("Nothing was written"), "{refused}");
    refused
}

fn rows(kernel: &Kernel, query: &str) -> Vec<BTreeMap<String, String>> {
    let json: serde_json::Value =
        serde_json::from_str(&select(kernel, query)).expect("the store answers JSON");
    json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|row| {
            row.as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v["value"].as_str().unwrap().to_string()))
                .collect()
        })
        .collect()
}

fn triple_count(kernel: &Kernel) -> usize {
    rows(
        kernel,
        &format!("SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{LIVE}> {{ ?s ?p ?o }} }}"),
    )[0]["n"]
        .parse()
        .unwrap()
}

/// One event of each class, and each edit's object.
fn whole_history() -> String {
    format!(
        r#"
<urn:src:1> a ledger:ClaimEvent ;
    dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime ;
    ledger:author <{ACTOR}> ;
    ledger:claimedBy "kata-ship/0abc" ;
    ledger:claimKind ledger:agent ;
    ledger:claimPrincipal <urn:example:principal> .
<urn:src:2> a ledger:ReleaseEvent ;
    dcterms:created "2020-02-02T00:00:00Z"^^xsd:dateTime ;
    ledger:claimedBy "kata-ship/0abc" ;
    ledger:releaseReason "released" ;
    ledger:claimedAt "2020-02-01T00:00:00Z"^^xsd:dateTime .
<urn:src:3> a ledger:EditEvent ;
    dcterms:created "2020-02-03T00:00:00Z"^^xsd:dateTime ;
    ledger:editKind "label" ; ledger:editLabel "bug" .
<urn:src:4> a ledger:EditEvent ;
    dcterms:created "2020-02-04T00:00:00Z"^^xsd:dateTime ;
    ledger:editKind "state" ;
    ledger:stateFrom <urn:iki:ledger:lifecycle:kata-flight:filed> ;
    ledger:stateTo <urn:iki:ledger:lifecycle:kata-flight:queued> .
<urn:src:5> a ledger:EditEvent ;
    dcterms:created "2020-02-05T00:00:00Z"^^xsd:dateTime ;
    ledger:editKind "link" ; ledger:editLinkType ledger:blocks ;
    ledger:editTarget <urn:iki:ledger:other:item:01abc> .
<urn:src:6> a ledger:EditEvent ;
    dcterms:created "2020-02-06T00:00:00Z"^^xsd:dateTime ;
    ledger:editKind "about" ; ledger:editTarget <urn:repo:file:x> .
<urn:src:7> a ledger:EditEvent ;
    dcterms:created "2020-02-07T00:00:00Z"^^xsd:dateTime ;
    ledger:editKind "edit" ; ledger:editField "title", "body" .
<urn:src:8> a ledger:EditEvent ;
    dcterms:created "2020-02-08T00:00:00Z"^^xsd:dateTime ;
    ledger:editKind "defer" .
<urn:src:9> a ledger:StatusEvent ;
    dcterms:created "2020-02-09T00:00:00Z"^^xsd:dateTime ;
    ledger:status ledger:closed ; ledger:closedReason ledger:done ;
    ledger:evidence <https://example.com/commit/abc> .
<urn:src:10> a ledger:StatusEvent ;
    dcterms:created "2020-02-10T00:00:00Z"^^xsd:dateTime ;
    ledger:status ledger:open .
"#
    )
}

/// Every face of the current state a reader or a loop acts on.
fn current_state(kernel: &Kernel) -> Vec<String> {
    let mut faces = Vec::new();
    for (resource, args) in [
        ("item:1", vec![]),
        ("item:1", vec![("as", "application/json")]),
        ("item:1", vec![("as", "text/turtle")]),
        ("items", vec![]),
        ("items", vec![("as", "application/json")]),
        ("item:1:state", vec![]),
        ("next", vec![]),
        ("counts", vec![]),
    ] {
        faces.push(source(kernel, &at(resource), &args));
    }
    for assertion in ["item:1:closed", "item:1:holder:none", "item:1:state:filed"] {
        faces.push(format!(
            "{assertion} {:?}",
            try_verb(kernel, Verb::Exists, &at(assertion), &[])
        ));
    }
    faces
}

// ------------------------------------------------------------------- recorded, state unchanged

/// ★ The acceptance's first line: the whole batch lands, and no face of the item moves.
#[test]
fn a_batch_is_recorded_and_the_current_state_does_not_move() {
    let kernel = seeded();
    let before = current_state(&kernel);
    let answer = import(&kernel, &whole_history());
    assert_eq!(
        answer,
        "acme#1: imported events recorded 10, already recorded 0; history only, so the item \
         itself did not change\n"
    );
    assert_eq!(current_state(&kernel), before);
    // Nothing the item carries as a subject was added: it has no state, label, claim or close.
    let item = &rows(
        &kernel,
        &format!("SELECT ?i WHERE {{ GRAPH <{LIVE}> {{ ?i <{NS}number> 1 }} }}"),
    )[0]["i"];
    let terms: Vec<String> = rows(
        &kernel,
        &format!("SELECT ?p WHERE {{ GRAPH <{LIVE}> {{ <{item}> ?p ?o }} }}"),
    )
    .into_iter()
    .map(|row| row["p"].clone())
    .collect();
    for held in [
        "state",
        "label",
        "claimedBy",
        "closedAt",
        "deferred",
        "blocks",
    ] {
        assert!(!terms.contains(&format!("{NS}{held}")), "{held}: {terms:?}");
    }
}

/// An event holds exactly its shape's terms, plus the ledger's own marks: the importer's committer
/// and the true time, beside the claimed date.
#[test]
fn an_event_says_who_recorded_it_and_when_beside_the_claimed_date() {
    let kernel = seeded();
    import(&kernel, &whole_history());
    let mut node: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows(
        &kernel,
        &format!(
            "SELECT ?p ?o WHERE {{ GRAPH <{LIVE}> {{ ?e <{NS}eventKey> \"urn:src:1\" ; ?p ?o }} }}"
        ),
    ) {
        node.entry(row["p"].rsplit(['#', '/']).next().unwrap().to_string())
            .or_default()
            .push(row["o"].clone());
    }
    let one = |term: &str| node[term].join(" | ");
    assert_eq!(one("created"), "2020-02-01T00:00:00Z");
    assert_eq!(one("createdSource"), "import");
    assert_eq!(one("committer"), IMPORTER);
    assert!(one("committedAt").starts_with("2026-09-15"), "{node:?}");
    assert_eq!(one("author"), ACTOR);
    assert_eq!(one("authorSource"), "import");
    assert_eq!(one("claimedBy"), "kata-ship/0abc");
    assert_eq!(one("claimKind"), format!("{NS}agent"));
    assert!(!node.contains_key("eventSequence"), "{node:?}");
}

/// A title holding the door's own grammar is ledger content, as everywhere else here.
#[test]
fn a_value_cannot_escape_the_update() {
    let kernel = seeded();
    let hostile = r#"x\") } ; DROP ALL ; INSERT DATA { <urn:a> <urn:b> \"c"#;
    import(
        &kernel,
        &format!(
            r#"<urn:src:1> a ledger:EditEvent ;
                dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime ;
                ledger:editKind "label" ; ledger:editLabel "{hostile}" ."#
        ),
    );
    let held = rows(
        &kernel,
        &format!("SELECT ?l WHERE {{ GRAPH <{LIVE}> {{ ?e <{NS}editLabel> ?l }} }}"),
    );
    assert_eq!(held[0]["l"], hostile.replace("\\\"", "\""));
    assert_eq!(
        rows(
            &kernel,
            &format!("SELECT ?i WHERE {{ GRAPH <{LIVE}> {{ ?i <{NS}number> 1 }} }}")
        )
        .len(),
        1
    );
}

// --------------------------------------------------------------------------------- idempotent

#[test]
fn a_re_send_changes_nothing() {
    let kernel = seeded();
    import(&kernel, &whole_history());
    let triples = triple_count(&kernel);
    let graph_query =
        format!("SELECT ?s ?p ?o WHERE {{ GRAPH <{LIVE}> {{ ?s ?p ?o }} }} ORDER BY ?s ?p ?o");
    let before = select(&kernel, &graph_query);
    let history = source(&kernel, &at("item:1:history"), &[]);
    assert_eq!(
        import(&kernel, &whole_history()),
        "acme#1: imported events recorded 0, already recorded 10; history only, so the item \
         itself did not change\n"
    );
    assert_eq!(triple_count(&kernel), triples);
    assert_eq!(source(&kernel, &at("item:1:history"), &[]), history);
    let content = format!("{PREFIXES}{}", whole_history());
    let replay = try_as(
        &kernel,
        &importer(),
        Verb::Sink,
        &at("history"),
        &[
            ("item", "1"),
            ("content", &content),
            ("as", "application/json"),
        ],
    )
    .unwrap();
    let replay: serde_json::Value = serde_json::from_str(&replay).unwrap();
    assert_eq!(replay["outcome"], "existing");
    assert_eq!(replay["recorded"], 0);
    assert_eq!(replay["already_recorded"], 10);
    assert_eq!(select(&kernel, &graph_query), before);
}

/// ★ As a keyed comment's key is, a source id is the ledger's: naming it again on another item,
/// or with another date, is the same event and changes nothing.
#[test]
fn a_source_id_is_taken_across_the_ledger_whatever_else_the_re_send_says() {
    let kernel = seeded();
    file(&kernel, "Another");
    import(
        &kernel,
        r#"<urn:src:1> a ledger:EditEvent ;
            dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime ; ledger:editKind "defer" ."#,
    );
    let triples = triple_count(&kernel);
    let on_another = send(
        &kernel,
        &importer(),
        "2",
        r#"<urn:src:1> a ledger:EditEvent ;
            dcterms:created "2020-03-01T00:00:00Z"^^xsd:dateTime ; ledger:editKind "resume" ."#,
    )
    .unwrap();
    assert!(
        on_another.contains("recorded 0, already recorded 1"),
        "{on_another}"
    );
    assert_eq!(triple_count(&kernel), triples);
}

#[test]
fn a_batch_that_is_partly_a_re_send_records_only_the_new_events() {
    let kernel = seeded();
    let event = |n: u32| {
        format!(
            r#"<urn:src:{n}> a ledger:EditEvent ;
                dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime ; ledger:editKind "defer" .
"#
        )
    };
    import(&kernel, &event(1));
    let answer = import(&kernel, &format!("{}{}{}", event(1), event(2), event(3)));
    assert!(
        answer.contains("recorded 2, already recorded 1"),
        "{answer}"
    );
    assert!(source(&kernel, &at("item:1:history"), &[]).starts_with("acme#1: 3 event(s)\n"));
}

/// ★ The race the read before the write cannot see: the same batch lands after that read and
/// before the update. The update's own guard is what records each event once.
#[test]
fn concurrent_re_sends_record_each_event_once() {
    let (kernel, racer) = racing_kernel();
    let batch = format!(
        r#"{PREFIXES}<urn:src:1> a ledger:EditEvent ;
            dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime ; ledger:editKind "defer" ."#
    );
    *racer.lock().unwrap() = Some(
        Request::new(Verb::Sink, Iri::parse(at("history")).unwrap())
            .with_arg("item", ArgRef::Inline(b"1".to_vec()))
            .with_arg("content", ArgRef::Inline(batch.clone().into_bytes())),
    );
    send(
        &kernel,
        &importer(),
        "1",
        batch.trim_start_matches(PREFIXES),
    )
    .unwrap();
    assert!(racer.lock().unwrap().is_none(), "the racer ran");
    assert_eq!(
        rows(
            &kernel,
            &format!("SELECT ?e WHERE {{ GRAPH <{LIVE}> {{ ?e <{NS}eventKey> \"urn:src:1\" }} }}")
        )
        .len(),
        1,
    );
}

/// What runs ahead of a kernel's next ledger update, once.
type Racer = Arc<Mutex<Option<Request>>>;

fn racing_kernel() -> (Kernel, Racer) {
    let racer: Racer = Arc::default();
    let ledger = ikigai_ledger::space_with(SpaceConfig::default().participants(PARTICIPANTS));
    let space = Fallback::new(vec![
        Arc::new(Interleave {
            inner: Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())),
            racer: Arc::clone(&racer),
        }) as Arc<dyn Space>,
        Arc::new(ledger) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    enroll(&kernel);
    file(&kernel, "Fix the widget");
    (kernel, racer)
}

/// Runs its [`Racer`] ahead of the first ledger update it sees.
struct Interleave {
    inner: Arc<dyn Space>,
    racer: Racer,
}

struct BeforeUpdate {
    inner: Arc<dyn Endpoint>,
    racer: Racer,
}

impl Space for Interleave {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let racer = Arc::clone(&self.racer);
        self.inner
            .resolve(request, scope)
            .map_endpoint(move |inner| Arc::new(BeforeUpdate { inner, racer }) as Arc<dyn Endpoint>)
    }
}

#[async_trait]
impl Endpoint for BeforeUpdate {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.target.as_str() == "urn:iki:store:graph-update" {
            let racer = self.racer.lock().unwrap().take();
            if let Some(racer) = racer {
                inv.issue(racer).await?;
            }
        }
        self.inner.invoke(inv).await
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn describe(&self) -> ikigai_core::Description {
        self.inner.describe()
    }
}

// -------------------------------------------------------------------------- anyone else refused

/// ★ With a sentence that says what to do instead, and before a byte of the batch is parsed.
#[test]
fn a_principal_nobody_enrolled_is_told_to_write_the_change_itself() {
    let kernel = seeded();
    let triples = triple_count(&kernel);
    let refused = send(&kernel, &dispatcher(), "1", &whole_history()).unwrap_err();
    assert!(matches!(refused, Error::Denied(_)), "{refused:?}");
    let refused = refused.to_string();
    assert!(
        refused.contains(&format!(
            "only an import or sync principal may record past-dated history, and {DISPATCHER} is \
             not one (ledger:Importer in <{PARTICIPANTS}>). Write the change itself instead"
        )),
        "{refused}"
    );
    assert_eq!(triple_count(&kernel), triples);
    // Not parsed: garbage gets the same answer.
    let garbage = send(&kernel, &dispatcher(), "1", "{{{")
        .unwrap_err()
        .to_string();
    assert!(garbage.contains("Write the change itself"), "{garbage}");
}

#[test]
fn root_carries_no_principal_and_is_refused_the_same_way() {
    let kernel = seeded();
    let refused = send(&kernel, &Capability::root(), "1", &whole_history())
        .unwrap_err()
        .to_string();
    assert!(refused.contains("carries no principal"), "{refused}");
    assert!(refused.contains("Write the change itself"), "{refused}");
}

/// Refused in the same sentence, where a raw answer from the store would name no way forward.
#[test]
fn a_principal_who_cannot_read_the_participants_graph_is_refused_the_same_way() {
    let kernel = seeded();
    let who = Capability::scoped(vec![
        format!("urn:cap:ledger:write:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
    ])
    .with_principal(IMPORTER)
    .unwrap();
    let refused = send(&kernel, &who, "1", &whole_history())
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("cannot read the participants graph"),
        "{refused}"
    );
    assert!(refused.contains("Write the change itself"), "{refused}");
}

#[test]
fn a_host_with_no_participants_graph_has_no_import_principal() {
    let kernel = common::kernel();
    append(&kernel, "An item", &[]);
    let who = Capability::scoped(vec![
        "urn:cap:ledger:write:default".to_string(),
        graph_read("default"),
        graph_write("default"),
    ])
    .with_principal(IMPORTER)
    .unwrap();
    let refused = try_as(
        &kernel,
        &who,
        Verb::Sink,
        "urn:iki:ledger:history",
        &[("item", "1"), ("content", &whole_history())],
    )
    .unwrap_err()
    .to_string();
    assert!(refused.contains("names no participants graph"), "{refused}");
}

#[test]
fn the_write_grant_comes_first_and_enrollment_does_not_stand_in_for_it() {
    let kernel = seeded();
    let who = Capability::scoped(grants(&["read", "claim", "comment"]))
        .with_principal(IMPORTER)
        .unwrap();
    let refused = send(&kernel, &who, "1", &whole_history())
        .unwrap_err()
        .to_string();
    assert!(refused.contains("urn:cap:ledger:write:"), "{refused}");
    let elsewhere = Capability::scoped({
        let mut scopes = grants(&["read"]);
        scopes.push("urn:cap:ledger:write:other".to_string());
        scopes
    })
    .with_principal(IMPORTER)
    .unwrap();
    let refused = send(&kernel, &elsewhere, "1", &whole_history())
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains(&format!("urn:cap:ledger:write:{LEDGER}")),
        "{refused}"
    );
}

// ------------------------------------------------------------------------------------- dates

fn dated(when: &str) -> String {
    format!(
        r#"<urn:src:1> a ledger:EditEvent ;
            dcterms:created {when} ; ledger:editKind "defer" ."#
    )
}

#[test]
fn a_date_after_now_is_refused() {
    let kernel = seeded();
    let refused = refusal(&kernel, &dated(r#""2030-01-01T00:00:00Z"^^xsd:dateTime"#));
    assert!(refused.contains("after this write"), "{refused}");
    assert!(refused.contains("urn:src:1"), "{refused}");
}

#[test]
fn a_date_before_the_filing_is_refused_and_the_filing_itself_is_not() {
    let kernel = seeded();
    let refused = refusal(&kernel, &dated(r#""2019-12-31T23:59:59Z"^^xsd:dateTime"#));
    assert!(refused.contains("before the item was filed"), "{refused}");
    let answer = import(&kernel, &dated(&format!("\"{FILED}\"^^xsd:dateTime")));
    assert!(answer.contains("recorded 1"), "{answer}");
}

#[test]
fn a_release_claimed_at_is_held_to_the_same_rules() {
    let kernel = seeded();
    let refused = refusal(
        &kernel,
        r#"<urn:src:1> a ledger:ReleaseEvent ;
            dcterms:created "2020-02-02T00:00:00Z"^^xsd:dateTime ;
            ledger:claimedBy "w" ; ledger:releaseReason "released" ;
            ledger:claimedAt "2019-02-01T00:00:00Z"^^xsd:dateTime ."#,
    );
    assert!(refused.contains("before the item was filed"), "{refused}");
}

#[test]
fn a_date_that_is_not_a_date_is_refused() {
    let kernel = seeded();
    for when in [
        r#""2020-02-01T00:00:00Z""#,
        r#""yesterday"^^xsd:dateTime"#,
        r#""2020-02-01"^^xsd:date"#,
        "<urn:x>",
    ] {
        let refused = refusal(&kernel, &dated(when));
        assert!(refused.contains("xsd:dateTime"), "{when}: {refused}");
    }
}

#[test]
fn one_bad_event_refuses_the_whole_batch() {
    let kernel = seeded();
    refusal(
        &kernel,
        &format!(
            "{}\n{}",
            dated(r#""2020-02-01T00:00:00Z"^^xsd:dateTime"#),
            dated(r#""2030-02-01T00:00:00Z"^^xsd:dateTime"#).replace("src:1", "src:2")
        ),
    );
}

// ------------------------------------------------------------------------------- closed shapes

/// ★ Each batch here would write something an event must not hold, or nothing a reader can use.
/// Every one is refused by name.
#[test]
fn what_a_shape_does_not_name_is_refused() {
    let kernel = seeded();
    let date = r#"dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime"#;
    for (why, event, says) in [
        (
            "a term of no shape",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "defer" ; ledger:title "x" ."#
            ),
            "is not a term this event takes",
        ),
        (
            "the sequence, which would reorder status events",
            format!(
                r#"<urn:s> a ledger:StatusEvent ; {date} ; ledger:status ledger:open ; ledger:eventSequence 1 ."#
            ),
            "eventSequence",
        ),
        (
            "a forged committer",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "defer" ; ledger:committer <urn:x> ."#
            ),
            "committer",
        ),
        (
            "another item",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "defer" ; ledger:onItem <urn:x> ."#
            ),
            "onItem",
        ),
        (
            "no class",
            format!(r#"<urn:s> {date} ."#),
            "it has one class",
        ),
        (
            "an item's class",
            format!(r#"<urn:s> a ledger:Item ; {date} ."#),
            "it has one class",
        ),
        (
            "two classes",
            format!(
                r#"<urn:s> a ledger:EditEvent, ledger:ClaimEvent ; {date} ; ledger:editKind "defer" ."#
            ),
            "it has one class",
        ),
        (
            "no date",
            r#"<urn:s> a ledger:EditEvent ; ledger:editKind "defer" ."#.to_string(),
            "exactly one",
        ),
        (
            "two dates",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date}, "2020-02-02T00:00:00Z"^^xsd:dateTime ; ledger:editKind "defer" ."#
            ),
            "exactly one",
        ),
        (
            "an unknown kind",
            format!(r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "rename" ."#),
            "one of label",
        ),
        (
            "an object of another kind",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "defer" ; ledger:editLabel "x" ."#
            ),
            "editLabel",
        ),
        (
            "a kind with no object",
            format!(r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "label" ."#),
            "editLabel",
        ),
        (
            "half a transition",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "state" ; ledger:stateTo <urn:q> ."#
            ),
            "stateFrom",
        ),
        (
            "a lifecycle version, since this ledger checked the move against nothing",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "state" ; ledger:stateFrom <urn:q> ; ledger:stateTo <urn:r> ; ledger:lifecycleVersion "1" ."#
            ),
            "lifecycleVersion",
        ),
        (
            "a field's value, which an edit event never holds",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "edit" ; ledger:editField "the new title" ."#
            ),
            "one of title",
        ),
        (
            "an unknown link type",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "link" ; ledger:editLinkType ledger:title ; ledger:editTarget <urn:x> ."#
            ),
            "editLinkType",
        ),
        (
            "evidence on a reopen",
            format!(
                r#"<urn:s> a ledger:StatusEvent ; {date} ; ledger:status ledger:open ; ledger:evidence <urn:x> ."#
            ),
            "evidence",
        ),
        (
            "an unknown status",
            format!(r#"<urn:s> a ledger:StatusEvent ; {date} ; ledger:status ledger:done ."#),
            "ledger#status",
        ),
        (
            "an unknown close reason",
            format!(
                r#"<urn:s> a ledger:StatusEvent ; {date} ; ledger:status ledger:closed ; ledger:closedReason ledger:open ."#
            ),
            "closedReason",
        ),
        (
            "an unknown release reason",
            format!(
                r#"<urn:s> a ledger:ReleaseEvent ; {date} ; ledger:claimedBy "w" ; ledger:releaseReason "bored" ."#
            ),
            "one of released",
        ),
        (
            "a claim with no holder",
            format!(r#"<urn:s> a ledger:ClaimEvent ; {date} ."#),
            "claimedBy",
        ),
        (
            "a free-text author",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "defer" ; ledger:author "octocat" ."#
            ),
            "an IRI",
        ),
        (
            "a holder across two lines, which would forge a history line",
            format!(
                r#"<urn:s> a ledger:ClaimEvent ; {date} ; ledger:claimedBy "a\n2020-01-01T00:00:00.000Z  closed" ."#
            ),
            "one line",
        ),
        (
            "an empty label",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "label" ; ledger:editLabel " " ."#
            ),
            "one line",
        ),
        (
            "a label in a language",
            format!(
                r#"<urn:s> a ledger:EditEvent ; {date} ; ledger:editKind "label" ; ledger:editLabel "bug"@en ."#
            ),
            "a plain string",
        ),
        (
            "a blank node",
            format!(r#"[] a ledger:EditEvent ; {date} ; ledger:editKind "defer" ."#),
            "blank node",
        ),
        (
            "a source id a key may not be",
            format!(
                r#"<urn:s/with/slashes> a ledger:EditEvent ; {date} ; ledger:editKind "defer" ."#
            ),
            "is keyed by its digest",
        ),
        ("not Turtle", "this is not turtle".to_string(), "not Turtle"),
        ("nothing", String::new(), "holds no events"),
    ] {
        let refused = refusal(&kernel, &event);
        assert!(refused.contains(says), "{why}: {refused}");
    }
}

/// `{ }` around a quoted term and a graph are neither of them Turtle events.
#[test]
fn a_quad_in_a_named_graph_is_not_turtle_here() {
    let kernel = seeded();
    let refused = refusal(&kernel, "<urn:g> { <urn:s> a ledger:EditEvent . }");
    assert!(refused.contains("not Turtle"), "{refused}");
}

// -------------------------------------------------------------------------------------- bounds

#[test]
fn a_batch_past_its_bytes_is_refused_before_it_is_parsed() {
    let kernel = seeded();
    let refused = refusal(&kernel, &format!("# {}\n", "x".repeat(520 * 1024)));
    assert!(refused.contains("a batch is at most 524288"), "{refused}");
    assert!(refused.contains("smaller batches"), "{refused}");
}

/// Nesting a triple term is what could outrun a parser's stack, and the store's scan stops it
/// before the parser runs. A blank node or a collection, as deep as the byte bound allows, is on
/// the parser's heap and is refused as the first blank node it meets.
#[test]
fn a_batch_nested_past_the_bound_is_refused_before_it_is_parsed() {
    let kernel = seeded();
    let terms = format!(
        "<urn:s> <urn:p> {}<urn:o>{} .",
        "<<( <urn:s> <urn:p> ".repeat(1_000),
        " )>>".repeat(1_000)
    );
    let refused = send(&kernel, &importer(), "1", &terms)
        .unwrap_err()
        .to_string();
    assert!(refused.contains("MAX_RDF_NESTING"), "{refused}");
    for (open, close) in [("[ <urn:q> ", " ]"), ("( ", " )")] {
        let depth = 400 * 1024 / (open.len() + close.len());
        let deep = format!(
            "<urn:s> <urn:p> {}1{} .",
            open.repeat(depth),
            close.repeat(depth)
        );
        let refused = send(&kernel, &importer(), "1", &deep)
            .unwrap_err()
            .to_string();
        assert!(refused.contains("Nothing was written"), "{refused}");
    }
}

fn many(n: usize) -> String {
    (0..n)
        .map(|k| {
            format!(
                r#"<urn:src:{k}> a ledger:EditEvent ; dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime ; ledger:editKind "defer" .
"#
            )
        })
        .collect()
}

#[test]
fn a_batch_past_its_events_is_refused_and_one_at_the_bound_is_recorded() {
    let kernel = seeded();
    let refused = refusal(&kernel, &many(129));
    assert!(refused.contains("holds 129 events"), "{refused}");
    let answer = import(&kernel, &many(128));
    assert!(
        answer.contains("recorded 128, already recorded 0"),
        "{answer}"
    );
}

/// ★ The largest event the shapes admit, at the batch's bound, still fits the store's update: or
/// is refused here, by name and before the write, rather than by the store with SPARQL in its text.
#[test]
fn a_batch_whose_update_would_not_fit_the_store_is_refused_by_name() {
    let kernel = seeded();
    let targets: Vec<String> = (0..8000)
        .map(|k| format!("<urn:example:long:target:{k:06}>"))
        .collect();
    let refused = refusal(
        &kernel,
        &format!(
            r#"<urn:s> a ledger:EditEvent ; dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime ;
                ledger:editKind "about" ; ledger:editTarget {} ."#,
            targets.join(", ")
        ),
    );
    assert!(refused.contains("store takes at most"), "{refused}");
}

#[test]
fn an_unknown_item_is_not_found_after_the_batch_is_checked() {
    let kernel = seeded();
    let refused = send(&kernel, &importer(), "99", &whole_history()).unwrap_err();
    assert!(matches!(refused, Error::NotFound(_)), "{refused:?}");
}

// --------------------------------------------------------------------------------------- faces

/// ★ Pinned byte for byte: the date is `(claimed)`, the event is `imported` and who did it at the
/// source stands beside who recorded it, all in words.
#[test]
fn the_plain_history_says_imported_in_words() {
    let kernel = seeded();
    import(&kernel, &whole_history());
    let history = source(&kernel, &at("item:1:history"), &[]);
    assert_eq!(
        history,
        format!(
            "acme#1: 10 event(s)\n\
             2020-02-01T00:00:00.000Z (claimed)  imported claim kata-ship/0abc  by {ACTOR} (claimed by an importer)  committer {IMPORTER}\n\
             2020-02-02T00:00:00.000Z (claimed)  imported release kata-ship/0abc (released)  by {IMPORTER}\n\
             2020-02-03T00:00:00.000Z (claimed)  imported label bug  by {IMPORTER}\n\
             2020-02-04T00:00:00.000Z (claimed)  imported state filed to queued  by {IMPORTER}\n\
             2020-02-05T00:00:00.000Z (claimed)  imported link blocks urn:iki:ledger:other:item:01abc  by {IMPORTER}\n\
             2020-02-06T00:00:00.000Z (claimed)  imported about urn:repo:file:x  by {IMPORTER}\n\
             2020-02-07T00:00:00.000Z (claimed)  imported edit title, body  by {IMPORTER}\n\
             2020-02-08T00:00:00.000Z (claimed)  imported defer  by {IMPORTER}\n\
             2020-02-09T00:00:00.000Z (claimed)  imported closed done, evidence https://example.com/commit/abc  by {IMPORTER}\n\
             2020-02-10T00:00:00.000Z (claimed)  imported reopened  by {IMPORTER}\n"
        )
    );
}

#[test]
fn the_json_history_says_imported_and_a_written_event_says_nothing_new() {
    let kernel = seeded();
    import(&kernel, &whole_history());
    try_as(
        &kernel,
        &dispatcher(),
        Verb::Sink,
        &at("label"),
        &[("item", "1"), ("content", "live")],
    )
    .unwrap();
    let doc: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        &at("item:1:history"),
        &[("as", "application/json")],
    ))
    .unwrap();
    let events = doc["events"].as_array().unwrap();
    assert_eq!(events.len(), 11);
    // This ledger checked an imported move against no lifecycle, so it does not speak of a version.
    let moved = events.iter().find(|e| e["kind"] == "state").unwrap();
    assert!(moved.get("lifecycle_version").is_none(), "{moved}");
    let first = &events[0];
    assert_eq!(first["imported"], true);
    assert_eq!(first["source_id"], "urn:src:1");
    assert_eq!(first["author"], ACTOR);
    assert_eq!(first["at"], "2020-02-01T00:00:00.000Z");
    assert_eq!(first["committer"], IMPORTER);
    assert!(first["committed"]
        .as_str()
        .unwrap()
        .starts_with("2026-09-15"));
    let live = events.last().unwrap();
    assert_eq!(live["kind"], "label");
    for added in ["imported", "source_id", "author", "committed"] {
        assert!(live.get(added).is_none(), "{added}: {live}");
    }
}

/// An importer's own events of one millisecond keep the batch's order, not an alphabet's.
#[test]
fn events_of_one_moment_read_in_the_order_the_batch_gave_them() {
    let kernel = seeded();
    let event = |source: &str, kind: &str| {
        format!(
            r#"<urn:src:{source}> a ledger:EditEvent ;
                dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime ; ledger:editKind "{kind}" .
"#
        )
    };
    import(
        &kernel,
        &format!("{}{}", event("z", "defer"), event("a", "resume")),
    );
    let history = source(&kernel, &at("item:1:history"), &[]);
    let defer = history.find("imported defer").unwrap();
    let resume = history.find("imported resume").unwrap();
    assert!(defer < resume, "{history}");
}

#[test]
fn the_answer_has_a_json_face() {
    let kernel = seeded();
    let content = format!("{PREFIXES}{}", many(2));
    let args = [
        ("item", "1"),
        ("content", content.as_str()),
        ("as", "application/json"),
    ];
    let first: serde_json::Value = serde_json::from_str(
        &try_as(&kernel, &importer(), Verb::Sink, &at("history"), &args).unwrap(),
    )
    .unwrap();
    assert_eq!(first["outcome"], "imported");
    assert_eq!(first["recorded"], 2);
    assert_eq!(first["already_recorded"], 0);
    let second: serde_json::Value = serde_json::from_str(
        &try_as(&kernel, &importer(), Verb::Sink, &at("history"), &args).unwrap(),
    )
    .unwrap();
    assert_eq!(second["outcome"], "existing");
    assert_eq!(second["recorded"], 0);
    assert_eq!(second["already_recorded"], 2);
}

// ------------------------------------------------------- moves nothing a mirror or a close reads

const CLOSE_AND_REOPEN: &str = r#"
<urn:src:c> a ledger:StatusEvent ;
    dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime ;
    ledger:status ledger:closed ; ledger:closedReason ledger:done .
<urn:src:o> a ledger:StatusEvent ;
    dcterms:created "2020-02-02T00:00:00Z"^^xsd:dateTime ; ledger:status ledger:open .
<urn:src:e> a ledger:EditEvent ;
    dcterms:created "2020-02-03T00:00:00Z"^^xsd:dateTime ; ledger:editKind "unlabel" ;
    ledger:editLabel "x" .
"#;

fn mirror_version(kernel: &Kernel, target: &str) -> i64 {
    rows(
        kernel,
        &format!(
            "SELECT ?v WHERE {{ GRAPH <urn:iki:ledger:graph:other> {{ ?m a <{NS}Mirror> ; \
             <{NS}onItem> <{target}> ; <{NS}mirrorVersion> ?v }} }}"
        ),
    )[0]["v"]
        .parse()
        .unwrap()
}

/// `acme#1` (the subject) blocks `other#1`, and the mirror holds its version.
fn linked() -> (Kernel, String) {
    let kernel = seeded();
    let target = sink(
        &kernel,
        "urn:iki:ledger:other:append",
        &[("content", "Blocked over there")],
    )
    .split_whitespace()
    .nth(1)
    .unwrap()
    .to_string();
    sink(
        &kernel,
        &at("link"),
        &[("item", "1"), ("content", &target), ("type", "blocks")],
    );
    (kernel, target)
}

/// ★ The hub's decision (ledger #2843): imported status and edit events are NOT in the version a
/// mirror compares. Importing writes no mirror and the next real change moves the version by one,
/// exactly as it would have with no import; each half fails if `across::snapshot` counts them.
#[test]
fn an_imported_event_moves_no_mirror_version() {
    let (control, control_target) = linked();
    sink(&control, &at("close"), &[("item", "1")]);
    let (kernel, target) = linked();
    let before = mirror_version(&kernel, &target);
    let next_there = source(&kernel, "urn:iki:ledger:other:next", &[]);

    import(&kernel, CLOSE_AND_REOPEN);
    assert_eq!(
        mirror_version(&kernel, &target),
        before,
        "importing writes no mirror"
    );
    assert_eq!(
        source(&kernel, "urn:iki:ledger:other:next", &[]),
        next_there,
        "an imported close of the blocker frees nothing"
    );

    sink(&kernel, &at("close"), &[("item", "1")]);
    assert_eq!(
        mirror_version(&kernel, &target),
        mirror_version(&control, &control_target),
        "the next real change lands on the version it would have without the import"
    );
    assert!(!source(&kernel, "urn:iki:ledger:other:next", &[]).contains("nothing is ready"));
}

/// An imported `link` event names a target and writes no edge and no mirror there.
#[test]
fn an_imported_link_event_writes_no_edge() {
    let kernel = seeded();
    let target = sink(
        &kernel,
        "urn:iki:ledger:other:append",
        &[("content", "Over there")],
    )
    .split_whitespace()
    .nth(1)
    .unwrap()
    .to_string();
    import(
        &kernel,
        &format!(
            r#"<urn:src:1> a ledger:EditEvent ;
                dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime ; ledger:editKind "link" ;
                ledger:editLinkType ledger:blocks ; ledger:editTarget <{target}> ."#
        ),
    );
    assert!(rows(
        &kernel,
        &format!(
            "SELECT ?l WHERE {{ {{ GRAPH <{LIVE}> {{ ?l a <{NS}Link> }} }} UNION \
             {{ GRAPH <urn:iki:ledger:graph:other> {{ ?l a <{NS}Mirror> }} }} }}"
        ),
    )
    .is_empty());
    assert!(source(&kernel, "urn:iki:ledger:other:next", &[]).contains("Over there"));
}

fn status_events(kernel: &Kernel) -> Vec<BTreeMap<String, String>> {
    rows(
        kernel,
        &format!(
            "SELECT ?e ?n ?status WHERE {{ GRAPH <{LIVE}> {{ ?e a <{NS}StatusEvent> ; \
             <{NS}status> ?status . OPTIONAL {{ ?e <{NS}eventSequence> ?n }} \
             FILTER NOT EXISTS {{ ?e <{NS}createdSource> ?source }} }} }} ORDER BY ?n"
        ),
    )
}

/// The sequence a real close takes counts real closes: otherwise history would reorder what the
/// daily counts and `standing` read.
#[test]
fn an_imported_status_event_takes_no_place_in_the_sequence() {
    let kernel = seeded();
    import(&kernel, CLOSE_AND_REOPEN);
    sink(&kernel, &at("close"), &[("item", "1")]);
    sink(&kernel, &at("reopen"), &[("item", "1")]);
    let sequence: Vec<String> = status_events(&kernel)
        .iter()
        .map(|event| event["n"].clone())
        .collect();
    assert_eq!(sequence, ["1", "2"]);
}

/// A close's evidence lands on the close that stands, whatever the imported history says about
/// the item: here an imported close, later in the sequence of dates than nothing and later by
/// date than the real one if dates were what ordered them.
#[test]
fn evidence_lands_on_the_close_that_stands_and_never_on_an_imported_one() {
    let kernel = seeded();
    sink(
        &kernel,
        &at("close"),
        &[("item", "1"), ("evidence", "urn:repo:commit:first")],
    );
    import(
        &kernel,
        r#"<urn:src:c> a ledger:StatusEvent ;
            dcterms:created "2021-01-01T00:00:00Z"^^xsd:dateTime ; ledger:status ledger:closed ."#,
    );
    sink(
        &kernel,
        &at("close"),
        &[("item", "1"), ("evidence", "urn:repo:commit:second")],
    );
    let on_imported = rows(
        &kernel,
        &format!(
            "SELECT ?x WHERE {{ GRAPH <{LIVE}> {{ ?e <{NS}eventKey> \"urn:src:c\" ; ?p ?x . \
             FILTER(?p = <{NS}evidence> || ?p = <{NS}closedReason>) }} }}"
        ),
    );
    assert!(on_imported.is_empty(), "{on_imported:?}");
    let real = status_events(&kernel);
    assert_eq!(real.len(), 1, "{real:?}");
    let evidence = rows(
        &kernel,
        &format!(
            "SELECT ?x WHERE {{ GRAPH <{LIVE}> {{ <{}> <{NS}evidence> ?x }} }} ORDER BY ?x",
            real[0]["e"]
        ),
    );
    assert_eq!(evidence.len(), 2, "{evidence:?}");
}

/// A close stamped straight into the store is preserved on reopen unless it has its event. An
/// imported event of the same moment is not that event.
#[test]
fn an_imported_close_does_not_pass_for_a_legacy_close_s_event() {
    let kernel = seeded();
    let item = rows(
        &kernel,
        &format!("SELECT ?i WHERE {{ GRAPH <{LIVE}> {{ ?i <{NS}number> 1 }} }}"),
    )[0]["i"]
        .clone();
    let when = "2020-06-01T00:00:00.000Z";
    import(
        &kernel,
        &format!(
            r#"<urn:src:c> a ledger:StatusEvent ;
                dcterms:created "{when}"^^xsd:dateTime ; ledger:status ledger:closed ."#
        ),
    );
    sink(
        &kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!(
                "DELETE {{ GRAPH <{LIVE}> {{ <{item}> <{NS}status> ?s }} }} \
                 INSERT {{ GRAPH <{LIVE}> {{ <{item}> <{NS}status> <{NS}closed> ; \
                 <{NS}closedAt> \"{when}\"^^<http://www.w3.org/2001/XMLSchema#dateTime> }} }} \
                 WHERE {{ GRAPH <{LIVE}> {{ <{item}> <{NS}status> ?s }} }}"
            ),
        )],
    );
    sink(&kernel, &at("reopen"), &[("item", "1")]);
    let closes: Vec<_> = status_events(&kernel)
        .into_iter()
        .filter(|event| event["status"] == format!("{NS}closed"))
        .collect();
    assert_eq!(
        closes.len(),
        1,
        "the legacy close kept its own event: {closes:?}"
    );
}

/// The daily series reads real closes only, whatever days the imported ones name.
#[test]
fn the_daily_counts_do_not_read_an_imported_close() {
    let kernel = seeded();
    let before = source(&kernel, &at("counts"), &[]);
    import(&kernel, CLOSE_AND_REOPEN);
    import(
        &kernel,
        r#"<urn:src:last> a ledger:StatusEvent ;
            dcterms:created "2022-02-01T00:00:00Z"^^xsd:dateTime ; ledger:status ledger:closed ."#,
    );
    assert_eq!(source(&kernel, &at("counts"), &[]), before);
}

/// `next` still offers an item whose imported history ends in a claim or a close.
#[test]
fn next_ignores_an_imported_claim_and_an_imported_close() {
    let kernel = seeded();
    import(&kernel, &whole_history());
    import(
        &kernel,
        r#"<urn:src:held> a ledger:ClaimEvent ;
            dcterms:created "2021-01-01T00:00:00Z"^^xsd:dateTime ; ledger:claimedBy "w" .
           <urn:src:shut> a ledger:StatusEvent ;
            dcterms:created "2021-01-02T00:00:00Z"^^xsd:dateTime ; ledger:status ledger:closed ."#,
    );
    assert!(source(&kernel, &at("next"), &[]).contains("Fix the widget"));
    assert!(try_as(
        &kernel,
        &dispatcher(),
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("content", "worker-1")]
    )
    .is_ok());
}

/// A delete owns imported events as it owns any other, and a source id is free again with them.
#[test]
fn a_delete_takes_the_imported_events_with_it() {
    let kernel = seeded();
    file(&kernel, "A survivor");
    import(&kernel, &whole_history());
    sink(&kernel, &at("close"), &[("item", "2")]);
    assert_eq!(
        rows(
            &kernel,
            &format!("SELECT ?e WHERE {{ GRAPH <{LIVE}> {{ ?e <{NS}eventKey> ?k }} }}")
        )
        .len(),
        10
    );
    delete(&kernel, &at("item:1"), &[]);
    assert!(rows(
        &kernel,
        &format!("SELECT ?e WHERE {{ GRAPH <{LIVE}> {{ ?e <{NS}eventKey> ?k }} }}")
    )
    .is_empty());
    let answer = send(&kernel, &importer(), "2", &whole_history()).unwrap();
    assert!(answer.contains("recorded 10"), "{answer}");
}

/// A delete landing after the batch's checks: the update's guard writes nothing, and the caller
/// is told, as any write on a deleted item is.
#[test]
fn a_batch_that_loses_the_race_to_a_delete_is_told_so() {
    let (kernel, racer) = racing_kernel();
    *racer.lock().unwrap() = Some(Request::new(
        Verb::Delete,
        Iri::parse(at("item:1")).unwrap(),
    ));
    let refused = send(&kernel, &root_importer(), "1", &whole_history()).unwrap_err();
    assert!(matches!(refused, Error::NotFound(_)), "{refused:?}");
    assert!(rows(
        &kernel,
        &format!("SELECT ?e WHERE {{ GRAPH <{LIVE}> {{ ?e <{NS}eventKey> ?k }} }}")
    )
    .is_empty());
}

/// The importer, also able to delete, so the racer's delete runs under the capability it inherits.
fn root_importer() -> Capability {
    let mut scopes = grants(&["read", "write", "delete"]);
    scopes.push(graveyard_write(LEDGER));
    scopes.push(session_read(LEDGER));
    scopes.push(session_write(LEDGER));
    Capability::scoped(scopes).with_principal(IMPORTER).unwrap()
}

// ---------------------------------------------------------------------------------------- scale

/// `n` events on `items` items through the door, in batches of 100, or as many as an item holds.
fn import_at_scale(kernel: &Kernel, items: usize, events: usize) -> f64 {
    for k in 1..items {
        file(kernel, &format!("Item {}", k + 1));
    }
    let started = Instant::now();
    let per_item = events / items;
    let who = importer();
    for item in 1..=items {
        let reference = item.to_string();
        for first in (0..per_item).step_by(100) {
            let batch: String = (first..(first + 100).min(per_item))
                .map(|k| {
                    format!(
                        r#"<urn:src:{item}:{k}> a ledger:EditEvent ; dcterms:created "2020-02-01T00:00:00Z"^^xsd:dateTime ; ledger:editKind "label" ; ledger:editLabel "l{k}" .
"#
                    )
                })
                .collect();
            send(kernel, &who, &reference, &batch).unwrap();
        }
    }
    started.elapsed().as_secs_f64()
}

fn timed(what: &str, f: impl FnOnce() -> String) -> f64 {
    let started = Instant::now();
    let answer = f();
    let seconds = started.elapsed().as_secs_f64();
    println!("{what}: {seconds:.3}s, {} bytes", answer.len());
    seconds
}

/// Runs in every build, on a size a debug build can afford: every event lands, and the reads a
/// loop depends on still answer.
#[test]
fn a_few_thousand_events_import_and_the_reads_still_answer() {
    let kernel = seeded();
    import_at_scale(&kernel, 10, 2_000);
    assert_eq!(
        rows(
            &kernel,
            &format!(
                "SELECT (COUNT(?e) AS ?n) WHERE {{ GRAPH <{LIVE}> {{ ?e <{NS}eventKey> ?k }} }}"
            )
        )[0]["n"],
        "2000"
    );
    assert!(!source(&kernel, &at("next"), &[]).contains("nothing is ready"));
    assert!(source(&kernel, &at("item:1:history"), &[]).starts_with("acme#1: 200 event(s)\n"));
}

/// An item with 245 imported events, just under the 250 dependents a delete names one by one
/// (ledger #2922), in a graph past a small copy bound: the update that inlines its quads and
/// names its dependents fits the store's text and algebra bounds, and a delete and a purge
/// succeed. (The item itself is about 2,300 quads, so the bound here is 5,000.)
#[test]
fn an_item_with_hundreds_of_events_is_deleted_and_purged_in_a_graph_past_the_bound() {
    let kernel = seeded_over(
        DurableStore::in_memory()
            .unwrap()
            .with_max_scoped_copy_quads(5_000),
    );
    for first in (0..245).step_by(100) {
        let batch: String = (first..(first + 100).min(245))
            .map(|k| event_of_kind(1, k))
            .collect();
        import(&kernel, &batch);
    }
    let mut data = String::new();
    for k in 0..2_000 {
        data.push_str(&format!(
            "<urn:iki:ledger:acme:item:bulk{k}> a <{NS}Item> ; <{NS}number> {} ; \
             <http://purl.org/dc/terms/title> \"t\" ; <{NS}status> <{NS}open> .\n",
            k + 10
        ));
    }
    sink(
        &kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!("INSERT DATA {{ GRAPH <{LIVE}> {{ {data} }} }}"),
        )],
    );
    assert!(triple_count(&kernel) > 10_000);
    let grant = Capability::scoped({
        let mut scopes = grants(&["read", "write", "delete", "purge"]);
        scopes.push(graveyard_write(LEDGER));
        scopes.push(session_read(LEDGER));
        scopes.push(session_write(LEDGER));
        scopes.push(graveyard_read(LEDGER));
        scopes
    })
    .with_principal(DISPATCHER)
    .unwrap();
    let deleted = try_as(&kernel, &grant, Verb::Delete, &at("item:1"), &[])
        .unwrap_or_else(|e| panic!("delete of an item with 245 events: {e}"));
    assert!(deleted.contains("deleted acme#1"), "{deleted}");
    let purged = try_as(
        &kernel,
        &grant,
        Verb::Delete,
        &at("purge"),
        &[("content", "1")],
    )
    .unwrap_or_else(|e| panic!("purge of it: {e}"));
    assert!(purged.contains("archived quad(s) destroyed"), "{purged}");
    assert!(rows(
        &kernel,
        &format!("SELECT ?e WHERE {{ GRAPH ?g {{ ?e <{NS}eventKey> ?k }} }}")
    )
    .is_empty());
}

/// A kernel like [`seeded`]'s over `store`.
fn seeded_over(store: DurableStore) -> Kernel {
    let ledger = ikigai_ledger::space_with(SpaceConfig::default().participants(PARTICIPANTS));
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(store)) as Arc<dyn Space>,
        Arc::new(ledger) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    enroll(&kernel);
    file(&kernel, "Fix the widget");
    kernel
}

/// ★ The measurement the item asks for. Not in the default run: `cargo test --release --test
/// imported_history -- --ignored --nocapture measure` prints the numbers the README states.
#[test]
#[ignore = "a measurement: run it in release, with --nocapture"]
fn measure_an_import_of_tens_of_thousands_of_events() {
    for (items, events, bound) in [
        (300usize, 30_000usize, None),
        (1, 30_000, None),
        (300, 30_000, Some(1_000_000)),
    ] {
        println!("\n--- {events} events on {items} items, copy bound {bound:?}");
        let store = DurableStore::in_memory().unwrap();
        let store = match bound {
            Some(bound) => store.with_max_scoped_copy_quads(bound),
            None => store,
        };
        let kernel = seeded_over(store);
        let grant = Capability::scoped({
            let mut scopes = grants(&["read", "write", "delete", "purge"]);
            scopes.push(graveyard_write(LEDGER));
            scopes.push(session_read(LEDGER));
            scopes.push(session_write(LEDGER));
            scopes.push(graveyard_read(LEDGER));
            scopes
        })
        .with_principal(DISPATCHER)
        .unwrap();
        let read = |resource: &str, args: &[(&str, &str)]| {
            try_as(&kernel, &grant, Verb::Source, &at(resource), args)
                .unwrap_or_else(|e| format!("refused: {e}"))
        };
        let write = |what: &str, verb: Verb, resource: &str, args: &[(&str, &str)]| {
            let started = Instant::now();
            let answer = try_as(&kernel, &grant, verb, &at(resource), args);
            println!(
                "{what}: {:.3}s, {}",
                started.elapsed().as_secs_f64(),
                match answer {
                    Ok(answer) => answer.trim().chars().take(60).collect::<String>(),
                    Err(refused) => format!("REFUSED {}", &refused.to_string()[..160]),
                }
            );
        };
        timed("baseline next, one item, no events", || read("next", &[]));
        let seconds = import_at_scale(&kernel, items, events);
        println!(
            "imported: {seconds:.1}s through the door, {:.0} events/s, {} triples in the graph",
            events as f64 / seconds,
            triple_count(&kernel)
        );
        timed("next", || read("next", &[]));
        timed("item read", || read("item:1", &[]));
        timed("item read, JSON", || {
            read("item:1", &[("as", "application/json")])
        });
        timed("items", || read("items", &[]));
        timed("counts", || read("counts", &[]));
        timed("doctor", || read("doctor", &[]));
        let history = read("item:1:history", &[]);
        println!("history answers: {}", &history[..history.len().min(120)]);
        timed("history", || read("item:1:history", &[]));
        let started = Instant::now();
        let again = send(&kernel, &importer(), "1", &whole_history()).unwrap();
        println!(
            "a batch of ten more: {:.3}s, {}",
            started.elapsed().as_secs_f64(),
            again.trim()
        );
        let started = Instant::now();
        let again = send(&kernel, &importer(), "1", &whole_history()).unwrap();
        println!(
            "and re-sent: {:.3}s, {}",
            started.elapsed().as_secs_f64(),
            again.trim()
        );
        write("append", Verb::Sink, "append", &[("content", "A new item")]);
        write(
            "comment",
            Verb::Sink,
            "comment",
            &[("item", "1"), ("content", "note")],
        );
        write(
            "label",
            Verb::Sink,
            "label",
            &[("item", "1"), ("content", "x")],
        );
        write(
            "claim",
            Verb::Sink,
            "claim",
            &[("item", "1"), ("content", "w")],
        );
        write("release", Verb::Delete, "claim", &[("item", "1")]);
        write("close", Verb::Sink, "close", &[("item", "1")]);
        write("reopen", Verb::Sink, "reopen", &[("item", "1")]);
        write(
            "purge of another item",
            Verb::Delete,
            "purge",
            &[("content", "2")],
        );
        write("delete", Verb::Delete, "item:1", &[]);
    }

    // The bound is the graph's, whatever filled it: filed items alone, and no import.
    println!("\n--- no imported events: 45,000 filed items");
    let kernel = seeded();
    for first in (0..45_000).step_by(400) {
        let mut data = String::new();
        for k in first..first + 400 {
            data.push_str(&format!(
                "<urn:iki:ledger:acme:item:bulk{k}> a <{NS}Item> ; <{NS}number> {} ; \
                 <http://purl.org/dc/terms/title> \"t\" ; <{NS}status> <{NS}open> ; \
                 <http://purl.org/dc/terms/created> \"2020-02-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> ; \
                 <http://purl.org/dc/terms/modified> \"2020-02-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> .\n",
                k + 10
            ));
        }
        sink(
            &kernel,
            "urn:iki:store:update",
            &[(
                "content",
                &format!("INSERT DATA {{ GRAPH <{LIVE}> {{ {data} }} }}"),
            )],
        );
    }
    println!("{} triples in the graph", triple_count(&kernel));
    println!(
        "delete: {:?}",
        try_verb(&kernel, Verb::Delete, &at("item:1"), &[])
            .map_err(|e| e.to_string()[..160].to_string())
    );
}

/// The ten event kinds of [`whole_history`], one per index, with this item's source IRIs and
/// dates.
fn event_of_kind(item: usize, k: usize) -> String {
    let day = 1 + k % 28;
    let when = format!(
        "\"2021-{:02}-{day:02}T00:00:00Z\"^^xsd:dateTime",
        1 + (k / 28) % 12
    );
    let body = match k % 10 {
        0 => format!(
            "a ledger:ClaimEvent ; dcterms:created {when} ; ledger:author <{ACTOR}> ; \
             ledger:claimedBy \"worker\" ; ledger:claimKind ledger:agent"
        ),
        1 => format!(
            "a ledger:ReleaseEvent ; dcterms:created {when} ; ledger:claimedBy \"worker\" ; \
             ledger:releaseReason \"released\""
        ),
        2 => format!(
            "a ledger:EditEvent ; dcterms:created {when} ; ledger:editKind \"label\" ; \
             ledger:editLabel \"l{k}\""
        ),
        3 => format!(
            "a ledger:EditEvent ; dcterms:created {when} ; ledger:editKind \"link\" ; \
             ledger:editLinkType ledger:blocks ; ledger:editTarget <urn:iki:ledger:acme:item:src{}>",
            item + 1
        ),
        4 => format!(
            "a ledger:StatusEvent ; dcterms:created {when} ; ledger:status ledger:closed ; \
             ledger:closedReason ledger:done"
        ),
        5 => format!(
            "a ledger:StatusEvent ; dcterms:created {when} ; ledger:status ledger:open"
        ),
        6 => format!(
            "a ledger:EditEvent ; dcterms:created {when} ; ledger:editKind \"edit\" ; \
             ledger:editField \"title\""
        ),
        7 => format!(
            "a ledger:EditEvent ; dcterms:created {when} ; ledger:editKind \"about\" ; \
             ledger:editTarget <urn:repo:file:x{k}>"
        ),
        8 => format!(
            "a ledger:EditEvent ; dcterms:created {when} ; ledger:editKind \"defer\""
        ),
        _ => format!(
            "a ledger:EditEvent ; dcterms:created {when} ; ledger:editKind \"label\" ; \
             ledger:editLabel \"m{k}\""
        ),
    };
    format!("<urn:src:{item}:{k}> {body} .\n")
}

/// ★ The import at ledger #2922's profile, in one ledger: 4,200 items, 12,461 comments and
/// about 37,600 past events in all, about 33,000 of them in one ledger. This builds that one
/// ledger at its share (3,690 items, 10,940 comments, 33,000 events; the rest live in other
/// ledgers, which are other graphs and cost this one nothing), then times `next` and `close`,
/// and a delete and a purge at the store's DEFAULT copy bound. Not in the default run:
/// `cargo test --release --test imported_history -- --ignored --nocapture profile`.
#[test]
#[ignore = "a measurement: run it in release, with --nocapture"]
fn measure_an_import_at_the_profile_of_ledger_2922() {
    const ITEMS: usize = 3_690;
    const COMMENTS: usize = 10_940;
    const EVENTS: usize = 33_000;
    let kernel = seeded_over(DurableStore::in_memory().unwrap());
    let who = importer();
    let grant = Capability::scoped({
        let mut scopes = grants(&["read", "write", "delete", "purge"]);
        scopes.push(graveyard_write(LEDGER));
        scopes.push(session_read(LEDGER));
        scopes.push(session_write(LEDGER));
        scopes.push(graveyard_read(LEDGER));
        scopes
    })
    .with_principal(DISPATCHER)
    .unwrap();

    let started = Instant::now();
    for k in 1..ITEMS {
        file(&kernel, &format!("Imported item {}", k + 1));
    }
    let items_s = started.elapsed().as_secs_f64();
    let started = Instant::now();
    for k in 0..COMMENTS {
        let item = (k % ITEMS + 1).to_string();
        try_as(
            &kernel,
            &who,
            Verb::Sink,
            &at("comment"),
            &[
                ("item", &item),
                ("content", &format!("imported note {k}")),
                ("created", "2021-03-01T00:00:00Z"),
            ],
        )
        .unwrap();
    }
    let comments_s = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let per_item = EVENTS / ITEMS;
    let extra = EVENTS % ITEMS;
    let mut sent = 0;
    for item in 1..=ITEMS {
        let n = per_item + usize::from(item <= extra);
        let batch: String = (0..n).map(|k| event_of_kind(item, k)).collect();
        send(&kernel, &who, &item.to_string(), &batch).unwrap();
        sent += n;
    }
    let events_s = started.elapsed().as_secs_f64();
    println!(
        "imported {ITEMS} items in {items_s:.1}s, {COMMENTS} comments in {comments_s:.1}s, \
         {sent} events in {events_s:.1}s ({:.0} a second); {} triples in the graph",
        sent as f64 / events_s,
        triple_count(&kernel)
    );

    let read = |resource: &str, args: &[(&str, &str)]| {
        try_as(&kernel, &grant, Verb::Source, &at(resource), args)
            .unwrap_or_else(|e| format!("refused: {e}"))
    };
    let write = |what: &str, verb: Verb, resource: &str, args: &[(&str, &str)]| {
        let started = Instant::now();
        let answer = try_as(&kernel, &grant, verb, &at(resource), args);
        println!(
            "{what}: {:.3}s, {}",
            started.elapsed().as_secs_f64(),
            match answer {
                Ok(answer) => answer.trim().lines().next().unwrap_or("").to_string(),
                Err(refused) => format!(
                    "REFUSED {}",
                    refused.to_string().chars().take(160).collect::<String>()
                ),
            }
        );
    };
    timed("next", || read("next", &[]));
    timed("next, again", || read("next", &[]));
    timed("item read", || read("item:17", &[]));
    timed("items", || read("items", &[]));
    timed("items, json", || {
        read("items", &[("as", "application/json")])
    });
    timed("counts", || read("counts", &[]));
    write("close #17", Verb::Sink, "close", &[("item", "17")]);
    write("reopen #17", Verb::Sink, "reopen", &[("item", "17")]);
    write("close #18", Verb::Sink, "close", &[("item", "18")]);
    write("delete #17", Verb::Delete, "item:17", &[]);
    write(
        "purge #17 (deleted)",
        Verb::Delete,
        "purge",
        &[("content", "17")],
    );
    write(
        "purge #19 (live)",
        Verb::Delete,
        "purge",
        &[("content", "19")],
    );
    println!("{} triples in the graph after", triple_count(&kernel));
}
