//! **Request tokens** (ledger #4059): `idempotency-key=` on comment and note, then on close,
//! reopen, defer and a claim's release, whose `content=` note was an unkeyed comment.
//!
//! The reproduction is ikigai-core's `docs/research/sink-idempotency/repro/`: a link delivers a
//! write, the write lands, and the answer is lost (`Error::Timeout`). An agent sends the same
//! request again. Without a token every one of these writes posts its comment twice. Under a
//! token the retry is answered with the lost answer, byte for byte, and writes nothing.

mod common;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Clock, Error, Fallback, Iri, Kernel, Request, Resend, Resolution, Scope,
    Space, Time, Verb,
};
use ikigai_store::DurableStore;

const TOKEN: &str = "retry-7f3a";

/// A clock a test moves: one second per reading, and as far as [`Hands::jump`] says.
#[derive(Clone)]
struct Hands(Arc<AtomicU64>);

impl Hands {
    fn jump(&self, by: Duration) {
        self.0.fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    }
}

impl Clock for Hands {
    fn now(&self) -> Time {
        Time::from_millis(self.0.fetch_add(1_000, Ordering::SeqCst))
    }
}

/// A kernel over a store and a ledger built with `config`, and its clock.
fn kernel_with(config: ikigai_ledger::SpaceConfig) -> (Kernel, Hands) {
    let hands = Hands(Arc::new(AtomicU64::new(1_795_000_000_000)));
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space_with(config)) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(hands.clone()));
    (kernel, hands)
}

/// The repro's transport: delivers every request, and loses the answer of one when told to.
struct LossyLink {
    kernel: Kernel,
    lose: AtomicBool,
    lost: Mutex<Vec<Vec<u8>>>,
}

impl LossyLink {
    fn new(kernel: Kernel) -> Self {
        LossyLink {
            kernel,
            lose: AtomicBool::new(false),
            lost: Mutex::new(Vec::new()),
        }
    }

    fn lose_next(&self) {
        self.lose.store(true, Ordering::SeqCst);
    }

    fn send(&self, request: Request, cap: &Capability) -> Result<Vec<u8>, Error> {
        let answer = block_on(self.kernel.issue(request, cap));
        if self.lose.swap(false, Ordering::SeqCst) {
            if let Ok(repr) = &answer {
                self.lost.lock().unwrap().push(repr.bytes.clone());
            }
            return Err(Error::Timeout(
                "the link dropped after the request was delivered".into(),
            ));
        }
        answer.map(|repr| repr.bytes)
    }

    /// Send, lose the answer, and send again as an agent does on a transient failure:
    /// the lost answer and the retry's.
    fn lose_and_retry(&self, request: &Request, cap: &Capability) -> (Vec<u8>, Vec<u8>) {
        self.lose_next();
        let first = self.send(request.clone(), cap);
        assert!(matches!(first, Err(Error::Timeout(_))), "{first:?}");
        let retry = self
            .send(request.clone(), cap)
            .unwrap_or_else(|e| panic!("the retry of {} failed: {e}", request.target));
        (
            self.lost.lock().unwrap().pop().expect("a lost answer"),
            retry,
        )
    }
}

fn request(verb: Verb, iri: &str, args: &[(&str, &str)]) -> Request {
    args.iter().fold(
        Request::new(verb, Iri::parse(iri).unwrap()),
        |r, (name, value)| r.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    )
}

fn with_token<'a>(args: &[(&'a str, &'a str)], token: &'a str) -> Vec<(&'a str, &'a str)> {
    let mut args = args.to_vec();
    args.push(("idempotency-key", token));
    args
}

/// Every grant on the default ledger, for `principal`, as a door mints one.
fn writer(principal: &str) -> Capability {
    let mut scopes: Vec<String> = ["read", "write", "close", "claim", "comment"]
        .iter()
        .map(|verb| format!("urn:cap:ledger:{verb}:default"))
        .collect();
    scopes.push(graph_read("default"));
    scopes.push(graph_write("default"));
    Capability::scoped(scopes)
        .with_principal(principal)
        .unwrap()
}

fn link() -> LossyLink {
    LossyLink::new(kernel_with(ikigai_ledger::SpaceConfig::default()).0)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// How many comments on item 1 say `body`, read from the store.
fn copies(kernel: &Kernel, body: &str) -> usize {
    let rows = select(
        kernel,
        &format!(
            "SELECT ?c WHERE {{ GRAPH <urn:iki:ledger:graph:default> {{ ?c a \
             <https://ikigai-rs.dev/ns/ledger#Comment> ; \
             <https://ikigai-rs.dev/ns/ledger#body> \"{body}\" }} }}"
        ),
    );
    rows.matches("urn:iki:ledger:default:comment:").count()
}

fn graph(kernel: &Kernel) -> String {
    select(
        kernel,
        "SELECT ?s ?p ?o WHERE { GRAPH <urn:iki:ledger:graph:default> { ?s ?p ?o } } ORDER BY ?s ?p ?o",
    )
}

fn filed(link: &LossyLink) {
    link.send(
        request(
            Verb::Sink,
            "urn:iki:ledger:append",
            &[("content", "An item")],
        ),
        &Capability::root(),
    )
    .unwrap();
}

#[test]
fn an_untokened_comment_retried_after_a_lost_answer_is_posted_twice() {
    // The repro's "two copies", still what a call without a token gets.
    let link = link();
    filed(&link);
    let comment = request(
        Verb::Sink,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", "repro-note-7f3a")],
    );
    link.lose_and_retry(&comment, &Capability::root());
    assert_eq!(copies(&link.kernel, "repro-note-7f3a"), 2);
}

#[test]
fn a_tokened_comment_retried_after_a_lost_answer_is_posted_once_and_answers_the_lost_bytes() {
    // The repro's comment case, flipped: one copy, and the retry's bytes equal the lost answer's.
    for face in ["text/plain", "application/json"] {
        let link = link();
        filed(&link);
        let args = with_token(
            &[
                ("item", "#1"),
                ("content", "repro-keyed-91c2"),
                ("as", face),
            ],
            TOKEN,
        );
        let comment = request(Verb::Sink, "urn:iki:ledger:comment", &args);
        let (lost, retry) = link.lose_and_retry(&comment, &Capability::root());
        assert_eq!(copies(&link.kernel, "repro-keyed-91c2"), 1, "{face}");
        assert_eq!(text(&retry), text(&lost), "{face}");
        assert!(!text(&retry).contains("existing"), "{}", text(&retry));
        // And a third send changes nothing in the graph.
        let before = graph(&link.kernel);
        assert_eq!(
            link.send(comment.clone(), &Capability::root()).unwrap(),
            lost
        );
        assert_eq!(graph(&link.kernel), before, "{face}");
    }
}

#[test]
fn a_tokened_note_retried_after_a_lost_answer_is_sent_once() {
    let link = link();
    filed(&link);
    let root = writer("urn:iki:test:dispatcher");
    link.send(
        request(
            Verb::Sink,
            "urn:iki:ledger:claim",
            &[("item", "#1"), ("content", "worker")],
        ),
        &root,
    )
    .unwrap();
    let args = with_token(
        &[("item", "#1"), ("content", "rebase first"), ("epoch", "1")],
        TOKEN,
    );
    let note = request(Verb::Sink, "urn:iki:ledger:note", &args);
    let (lost, retry) = link.lose_and_retry(&note, &root);
    assert_eq!(retry, lost);
    assert_eq!(copies(&link.kernel, "rebase first"), 1);
    // ★ note and comment share one namespace: the same token on a comment is a different
    // request, refused, and nothing is written.
    let err = link
        .send(
            request(
                Verb::Sink,
                "urn:iki:ledger:comment",
                &with_token(&[("item", "#1"), ("content", "rebase first")], TOKEN),
            ),
            &root,
        )
        .unwrap_err();
    assert!(
        matches!(&err, Error::InvalidArgument { name, detail } if name == "idempotency-key" && detail.contains("was used for a different request")),
        "{err}"
    );
    assert_eq!(copies(&link.kernel, "rebase first"), 1);
}

#[test]
fn every_verb_with_a_content_note_posts_its_note_once_under_a_token() {
    // (setup, the verb's request) for each verb whose `content=` note was an unkeyed comment.
    type Setup = fn(&LossyLink);
    let open: Setup = |_| {};
    let closed: Setup = |link| {
        link.send(
            request(Verb::Sink, "urn:iki:ledger:close", &[("item", "#1")]),
            &Capability::root(),
        )
        .unwrap();
    };
    let deferred: Setup = |link| {
        link.send(
            request(Verb::Sink, "urn:iki:ledger:defer", &[("item", "#1")]),
            &Capability::root(),
        )
        .unwrap();
    };
    let claimed: Setup = |link| {
        link.send(
            request(
                Verb::Sink,
                "urn:iki:ledger:claim",
                &[("item", "#1"), ("content", "worker")],
            ),
            &Capability::root(),
        )
        .unwrap();
    };
    type Case<'a> = (&'a str, Setup, Verb, &'a str, &'a [(&'a str, &'a str)]);
    let cases: [Case; 8] = [
        ("close", claimed, Verb::Sink, "urn:iki:ledger:close", &[]),
        (
            "close json",
            open,
            Verb::Sink,
            "urn:iki:ledger:close",
            &[("as", "application/json")],
        ),
        ("reopen", closed, Verb::Sink, "urn:iki:ledger:reopen", &[]),
        (
            "reopen open",
            open,
            Verb::Sink,
            "urn:iki:ledger:reopen",
            &[],
        ),
        ("defer", open, Verb::Sink, "urn:iki:ledger:defer", &[]),
        (
            "resume",
            deferred,
            Verb::Delete,
            "urn:iki:ledger:defer",
            &[],
        ),
        (
            "release",
            claimed,
            Verb::Delete,
            "urn:iki:ledger:claim",
            &[],
        ),
        (
            "release by name",
            claimed,
            Verb::Delete,
            "urn:iki:ledger:claim",
            &[("from", "worker")],
        ),
    ];
    for (case, setup, verb, iri, extra) in cases {
        let note = format!("the note of {case}");
        // Without a token: the note twice (the gap this closes), or, for a release `from=` a
        // holder, the retry refused (Conflict) for work that succeeded, its answer lost.
        let bare = link();
        filed(&bare);
        setup(&bare);
        let mut args = vec![("item", "#1"), ("content", note.as_str())];
        args.extend_from_slice(extra);
        let untokened = request(verb, iri, &args);
        bare.lose_next();
        assert!(bare.send(untokened.clone(), &Capability::root()).is_err());
        let retry = bare.send(untokened, &Capability::root());
        if extra.iter().any(|(name, _)| *name == "from") {
            assert!(
                matches!(retry, Err(Error::Conflict(_))),
                "{case}: {retry:?}"
            );
            assert_eq!(copies(&bare.kernel, &note), 1, "{case} without a token");
        } else {
            retry.unwrap();
            assert_eq!(copies(&bare.kernel, &note), 2, "{case} without a token");
        }

        let link = link();
        filed(&link);
        setup(&link);
        let tokened = request(verb, iri, &with_token(&args, TOKEN));
        let (lost, retry) = link.lose_and_retry(&tokened, &Capability::root());
        assert_eq!(copies(&link.kernel, &note), 1, "{case}");
        assert_eq!(text(&retry), text(&lost), "{case}");
        let before = graph(&link.kernel);
        assert_eq!(
            link.send(tokened, &Capability::root()).unwrap(),
            lost,
            "{case}"
        );
        assert_eq!(
            graph(&link.kernel),
            before,
            "{case}: a replay writes nothing"
        );
    }
}

#[test]
fn a_verb_without_a_note_answers_its_first_answer_under_a_token() {
    // The answer a retried close without a token gets says it was already closed; under a token
    // it is the first answer.
    let link = link();
    filed(&link);
    let close = request(
        Verb::Sink,
        "urn:iki:ledger:close",
        &with_token(&[("item", "#1")], TOKEN),
    );
    let (lost, retry) = link.lose_and_retry(&close, &Capability::root());
    assert_eq!(text(&retry), text(&lost));
    assert!(!text(&retry).contains("already closed"), "{}", text(&retry));
}

#[test]
fn a_token_is_scoped_to_its_principal() {
    let link = link();
    filed(&link);
    let args = with_token(&[("item", "#1"), ("content", "the same words")], TOKEN);
    let comment = request(Verb::Sink, "urn:iki:ledger:comment", &args);
    let a = writer("urn:iki:test:a");
    let b = writer("urn:iki:test:b");
    let first = text(&link.send(comment.clone(), &a).unwrap());
    let second = text(&link.send(comment.clone(), &b).unwrap());
    assert_ne!(first, second, "b is not handed a's comment");
    assert_eq!(copies(&link.kernel, "the same words"), 2);
    assert_eq!(text(&link.send(comment, &a).unwrap()), first);
}

#[test]
fn a_token_reused_for_a_different_request_is_refused_and_writes_nothing() {
    let link = link();
    filed(&link);
    let first = request(
        Verb::Sink,
        "urn:iki:ledger:comment",
        &with_token(&[("item", "#1"), ("content", "first words")], TOKEN),
    );
    link.send(first, &Capability::root()).unwrap();
    let before = graph(&link.kernel);
    for args in [
        &[("item", "#1"), ("content", "other words")][..],
        &[
            ("item", "#1"),
            ("content", "first words"),
            ("as", "application/json"),
        ][..],
    ] {
        let err = link
            .send(
                request(
                    Verb::Sink,
                    "urn:iki:ledger:comment",
                    &with_token(args, TOKEN),
                ),
                &Capability::root(),
            )
            .unwrap_err();
        assert!(
            matches!(&err, Error::InvalidArgument { name, .. } if name == "idempotency-key"),
            "{err}"
        );
        assert!(
            err.to_string().contains("was used for a different request"),
            "{err}"
        );
    }
    assert_eq!(graph(&link.kernel), before);
}

#[test]
fn a_malformed_token_is_refused_before_anything_is_written() {
    let link = link();
    filed(&link);
    let before = graph(&link.kernel);
    for token in ["", "has space", &"k".repeat(257)] {
        let err = link
            .send(
                request(
                    Verb::Sink,
                    "urn:iki:ledger:comment",
                    &with_token(&[("item", "#1"), ("content", "words")], token),
                ),
                &Capability::root(),
            )
            .unwrap_err();
        assert!(
            matches!(&err, Error::InvalidArgument { name, .. } if name == "idempotency-key"),
            "{token:?}: {err}"
        );
    }
    assert_eq!(graph(&link.kernel), before);
}

#[test]
fn a_conflict_is_recorded_and_a_refusal_before_the_write_is_not() {
    let link = link();
    filed(&link);
    // A fenced comment at an epoch the item is not claimed at: a Conflict, recorded, so its
    // retry answers the same Conflict even once the claim exists.
    let fenced = request(
        Verb::Sink,
        "urn:iki:ledger:comment",
        &with_token(
            &[("item", "#1"), ("content", "fenced"), ("epoch", "1")],
            TOKEN,
        ),
    );
    let first = link.send(fenced.clone(), &Capability::root()).unwrap_err();
    assert!(matches!(first, Error::Conflict(_)), "{first}");
    link.send(
        request(
            Verb::Sink,
            "urn:iki:ledger:claim",
            &[("item", "#1"), ("content", "worker")],
        ),
        &Capability::root(),
    )
    .unwrap();
    let again = link.send(fenced, &Capability::root()).unwrap_err();
    assert_eq!(again.to_string(), first.to_string());
    assert_eq!(copies(&link.kernel, "fenced"), 0);

    // An invalid argument is a refusal before the write: not recorded, so the same request
    // runs again (and is refused again), and no record is left behind.
    let invalid = request(
        Verb::Sink,
        "urn:iki:ledger:comment",
        &with_token(
            &[
                ("item", "#1"),
                ("content", "both"),
                ("handback", "true"),
                ("rejects", "true"),
            ],
            "another-token",
        ),
    );
    for _ in 0..2 {
        let err = link.send(invalid.clone(), &Capability::root()).unwrap_err();
        assert!(matches!(err, Error::InvalidArgument { .. }), "{err}");
    }
    let records = select(
        &link.kernel,
        "SELECT ?r WHERE { GRAPH <urn:iki:ledger:graph:default> { ?r a \
         <https://ikigai-rs.dev/ns/ledger#Replay> } }",
    );
    assert_eq!(
        records.matches("urn:iki:ledger:default:replay:").count(),
        1,
        "{records}"
    );
}

#[test]
fn a_token_after_its_window_is_a_new_request() {
    let (kernel, hands) = kernel_with(
        ikigai_ledger::SpaceConfig::default().resend_window(Duration::from_secs(48 * 3600)),
    );
    let link = LossyLink::new(kernel);
    filed(&link);
    let comment = request(
        Verb::Sink,
        "urn:iki:ledger:comment",
        &with_token(&[("item", "#1"), ("content", "windowed")], TOKEN),
    );
    let first = link.send(comment.clone(), &Capability::root()).unwrap();
    hands.jump(Duration::from_secs(47 * 3600));
    assert_eq!(
        link.send(comment.clone(), &Capability::root()).unwrap(),
        first
    );
    assert_eq!(copies(&link.kernel, "windowed"), 1);
    hands.jump(Duration::from_secs(2 * 3600));
    let after = link.send(comment.clone(), &Capability::root()).unwrap();
    assert_ne!(after, first);
    assert_eq!(copies(&link.kernel, "windowed"), 2);
    // And the new answer is the token's record from then on.
    assert_eq!(link.send(comment, &Capability::root()).unwrap(), after);
    assert_eq!(copies(&link.kernel, "windowed"), 2);
}

#[test]
fn a_record_goes_with_its_item_on_a_purge() {
    let link = link();
    filed(&link);
    let comment = request(
        Verb::Sink,
        "urn:iki:ledger:comment",
        &with_token(
            &[
                ("item", "#1"),
                ("content", "a secret"),
                ("as", "application/json"),
            ],
            TOKEN,
        ),
    );
    link.send(comment, &Capability::root()).unwrap();
    let kernel = &link.kernel;
    delete(kernel, "urn:iki:ledger:purge", &[("content", "1")]);
    let left = select(
        kernel,
        "SELECT ?s WHERE { GRAPH ?g { ?s a <https://ikigai-rs.dev/ns/ledger#Replay> } }",
    );
    assert!(!left.contains("replay:"), "{left}");
}

#[test]
fn the_contract_declares_the_token_and_its_window() {
    let window = Duration::from_secs(72 * 3600);
    let space =
        ikigai_ledger::space_with(ikigai_ledger::SpaceConfig::default().resend_window(window));
    for (iri, verb) in [
        ("urn:iki:ledger:comment", Verb::Sink),
        ("urn:iki:ledger:note", Verb::Sink),
        ("urn:iki:ledger:decision", Verb::Sink),
        ("urn:iki:ledger:close", Verb::Sink),
        ("urn:iki:ledger:reopen", Verb::Sink),
        ("urn:iki:ledger:defer", Verb::Sink),
        ("urn:iki:ledger:defer", Verb::Delete),
        ("urn:iki:ledger:claim", Verb::Delete),
    ] {
        let Resolution::Hit(resolved) = space.resolve(
            &Request::new(Verb::Meta, Iri::parse(iri).unwrap()),
            &Scope::empty(),
        ) else {
            panic!("unresolved {iri}")
        };
        let action = resolved
            .endpoint
            .describe()
            .action_specs()
            .into_iter()
            .find(|a| a.verb == verb)
            .unwrap();
        assert_eq!(
            action.resend,
            Some(Resend::Key("idempotency-key".into())),
            "{iri}"
        );
        assert_eq!(action.resend_window_secs, Some(window.as_secs()), "{iri}");
        assert!(
            action.inputs.iter().any(|i| i.name == "idempotency-key"),
            "{iri}"
        );
    }
    let kernel = kernel();
    let meta = ok(&kernel, Verb::Meta, "urn:iki:ledger:comment", &[]);
    assert!(meta.contains("ik:resendWindow"), "{meta}");
    assert!(meta.contains(":input:idempotency-key>"), "{meta}");
}
