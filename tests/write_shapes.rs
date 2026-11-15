//! **Write shapes** (ledger #3724): a SHACL shape, configured per ledger, checked against a
//! write's would-be graph before it lands.
//!
//! - a refusing shape blocks a close without evidence, and lets one with evidence through;
//! - a warning shape lets the close through and records the firing on the trace;
//! - a shape that fails to load refuses nothing, and says so on the trace;
//! - a hand-back is checked as a `report`;
//! - a ledger without shapes behaves exactly as today, and issues no validation.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Description, EndpointSpace, Error, Exact, Fallback, FnEndpoint, Iri,
    Kernel, ReprType, Representation, Request, Space, TraceEvent, Tracer, Verb,
};
use ikigai_ledger::shapes::{UNLOADED_NOTE, WARN_NOTE};
use ikigai_ledger::{ShapeOutcome, SpaceConfig, WriteKind};
use ikigai_store::DurableStore;

const SHAPE: &str = "urn:test:shape:close-carries-evidence";
const HANDBACK_SHAPE: &str = "urn:test:shape:handback-names-a-pr";
const MISSING: &str = "urn:test:shape:missing";
const CLOSE: &str = "urn:iki:ledger:close";

const CLOSE_TTL: &str = r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .
<urn:test:shape:close-carries-evidence> a sh:NodeShape ;
  sh:targetClass ledger:StatusEvent ;
  sh:property [ sh:path ledger:evidence ; sh:minCount 1 ;
                sh:message "a close carries evidence" ] ."#;

const HANDBACK_TTL: &str = r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .
<urn:test:shape:handback-names-a-pr> a sh:NodeShape ;
  sh:targetClass ledger:Comment ;
  sh:property [ sh:path ledger:body ; sh:pattern "/pull/[0-9]+" ;
                sh:message "a hand-back names a pull request" ] ."#;

fn turtle(text: &'static str) -> FnEndpoint {
    FnEndpoint::new("shape", move |_| {
        Ok(Representation::new(
            ReprType::new("text/turtle"),
            text.as_bytes().to_vec(),
        ))
    })
    .with_description(Description::new("shape").verb(Verb::Source))
}

/// A kernel with the store, the validator, the two shapes as resources, and the ledger.
fn kernel_with(config: SpaceConfig) -> Kernel {
    let shapes = EndpointSpace::new()
        .bind(Exact::new(SHAPE), turtle(CLOSE_TTL))
        .bind(Exact::new(HANDBACK_SHAPE), turtle(HANDBACK_TTL));
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(
            DurableStore::in_memory().expect("an in-memory store"),
        )) as Arc<dyn Space>,
        Arc::new(ikigai_shacl::space()) as Arc<dyn Space>,
        Arc::new(shapes) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space_with(config)) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(TickingClock::default()))
}

#[derive(Default)]
struct Trace(Mutex<Vec<TraceEvent>>);
impl Tracer for Trace {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Trace {
    /// Every note under `key`, on any span.
    fn notes(&self, key: &str) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .flat_map(|e| e.notes.iter())
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .collect()
    }

    /// Whether any span resolved `iri`.
    fn resolved(&self, iri: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.target.as_str() == iri)
    }
}

/// Sink `iri` under root with a tracer, returning the result and the trace.
fn traced(
    kernel: &Kernel,
    iri: &str,
    args: &[(&str, &str)],
) -> (Result<String, Error>, Arc<Trace>) {
    let request = args.iter().fold(
        Request::new(Verb::Sink, Iri::parse(iri).unwrap()),
        |r, (name, value)| r.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    );
    let trace = Arc::new(Trace::default());
    let result = block_on(kernel.issue_traced(request, &Capability::root(), trace.clone()))
        .map(|repr| String::from_utf8_lossy(&repr.bytes).into_owned());
    (result, trace)
}

fn is_closed(kernel: &Kernel, number: &str) -> bool {
    source(
        kernel,
        &format!("urn:iki:ledger:item:{}", number.trim_start_matches('#')),
        &[],
    )
    .contains("closed")
}

#[test]
fn a_refusing_shape_blocks_a_close_without_evidence() {
    let kernel = kernel_with(SpaceConfig::default().write_shape(
        "default",
        SHAPE,
        &[WriteKind::Close],
        ShapeOutcome::Refuse,
    ));
    let number = append(&kernel, "Fix the thing", &[]);
    let (result, _) = traced(&kernel, CLOSE, &[("item", &number)]);
    let error = result.expect_err("a close without evidence is refused");
    let words = error.to_string();
    assert!(matches!(error, Error::Conflict(_)), "{error:?}");
    assert!(
        words.contains(SHAPE),
        "the refusal names the shape: {words}"
    );
    assert!(
        words.contains("MinCount on evidence"),
        "and the constraint: {words}"
    );
    assert!(
        words.contains("a close carries evidence"),
        "in words: {words}"
    );
    assert!(words.contains("Nothing was changed"), "{words}");
    assert!(!is_closed(&kernel, &number), "nothing was written");

    // The same close with evidence conforms, and lands.
    sink(
        &kernel,
        CLOSE,
        &[
            ("item", &number),
            ("evidence", "https://github.com/o/r/pull/9"),
        ],
    );
    assert!(is_closed(&kernel, &number));
}

#[test]
fn a_warning_shape_lets_the_close_through_and_records_the_firing() {
    let kernel = kernel_with(SpaceConfig::default().write_shape(
        "default",
        SHAPE,
        &[WriteKind::Close],
        ShapeOutcome::Warn,
    ));
    let number = append(&kernel, "Fix the thing", &[]);
    let (result, trace) = traced(&kernel, CLOSE, &[("item", &number)]);
    result.expect("a warning shape lets the close land");
    assert!(is_closed(&kernel, &number));
    let fired = trace.notes(WARN_NOTE);
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert!(
        fired[0].starts_with(&format!("{SHAPE} close: ")),
        "names the control: {fired:?}"
    );
    assert!(fired[0].contains("MinCount on evidence"), "{fired:?}");
}

#[test]
fn a_shape_that_fails_to_load_refuses_nothing_and_says_so() {
    let kernel = kernel_with(SpaceConfig::default().write_shape(
        "default",
        MISSING,
        &[WriteKind::Close],
        ShapeOutcome::Refuse,
    ));
    let number = append(&kernel, "Fix the thing", &[]);
    let (result, trace) = traced(&kernel, CLOSE, &[("item", &number)]);
    result.expect("a shape that cannot load refuses nothing");
    assert!(is_closed(&kernel, &number));
    let unloaded = trace.notes(UNLOADED_NOTE);
    assert_eq!(unloaded.len(), 1, "{unloaded:?}");
    assert!(
        unloaded[0].starts_with(&format!("{MISSING} close: ")),
        "{unloaded:?}"
    );
}

#[test]
fn a_hand_back_is_checked_as_a_report_and_a_plain_comment_is_not() {
    let kernel = kernel_with(SpaceConfig::default().write_shape(
        "default",
        HANDBACK_SHAPE,
        &[WriteKind::Report],
        ShapeOutcome::Refuse,
    ));
    let number = append(&kernel, "Fix the thing", &[]);
    // A comment that is not a hand-back is not governed.
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[("item", &number), ("content", "looking")],
    );
    let (result, _) = traced(
        &kernel,
        "urn:iki:ledger:comment",
        &[
            ("item", &number),
            ("content", "done, see the branch"),
            ("handback", "true"),
        ],
    );
    let words = result
        .expect_err("a hand-back naming no PR is refused")
        .to_string();
    assert!(words.contains("this report of"), "{words}");
    assert!(
        words.contains("a hand-back names a pull request"),
        "{words}"
    );
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[
            ("item", &number),
            ("content", "done: https://github.com/o/r/pull/9"),
            ("handback", "true"),
        ],
    );
}

#[test]
fn a_ledger_without_shapes_behaves_as_today() {
    // A shape on ANOTHER ledger governs nothing here.
    let kernel = kernel_with(SpaceConfig::default().write_shape(
        "acme",
        SHAPE,
        &[WriteKind::Close],
        ShapeOutcome::Refuse,
    ));
    let number = append(&kernel, "Fix the thing", &[]);
    let (result, trace) = traced(&kernel, CLOSE, &[("item", &number)]);
    result.expect("no shape governs the default ledger");
    assert!(is_closed(&kernel, &number));
    assert!(
        !trace.resolved("urn:shacl:validate"),
        "no validation is issued"
    );
    assert!(!trace.resolved(SHAPE), "no shape is read");
    assert!(trace.notes(WARN_NOTE).is_empty() && trace.notes(UNLOADED_NOTE).is_empty());

    // And the default configuration has no shapes at all.
    let plain = common::kernel();
    let number = append(&plain, "Fix the thing", &[]);
    sink(&plain, CLOSE, &[("item", &number)]);
}

#[test]
#[should_panic(expected = "governs no kind of write")]
fn a_shape_with_no_kinds_fails_at_the_manifest() {
    let _ = SpaceConfig::default().write_shape("default", SHAPE, &[], ShapeOutcome::Warn);
}

// `claim` and `label` are not checked yet, so a shape that lists them would look enforced and do
// nothing. It is refused where the manifest is built instead.
#[test]
#[should_panic(
    expected = "governs `claim`, and the ledger does not check a claim against a shape yet"
)]
fn a_shape_governing_claims_is_refused_at_the_manifest() {
    let _ = SpaceConfig::default().write_shape(
        "default",
        SHAPE,
        &[WriteKind::Close, WriteKind::Claim],
        ShapeOutcome::Refuse,
    );
}

#[test]
#[should_panic(
    expected = "governs `label`, and the ledger does not check a label against a shape yet"
)]
fn a_shape_governing_labels_is_refused_at_the_manifest() {
    let _ = SpaceConfig::default().write_shape(
        "default",
        SHAPE,
        &[WriteKind::Label],
        ShapeOutcome::Warn,
    );
}

#[test]
fn only_report_close_and_comment_are_checked() {
    let checked: Vec<_> = [
        WriteKind::Claim,
        WriteKind::Report,
        WriteKind::Close,
        WriteKind::Label,
        WriteKind::Comment,
    ]
    .into_iter()
    .filter(|k| k.is_checked())
    .map(WriteKind::name)
    .collect();
    assert_eq!(checked, ["report", "close", "comment"]);
}
