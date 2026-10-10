//! A ledger host is never network egress through SPARQL `SERVICE` or `LOAD`, in any build
//! (ledger #1085, the follow-on to #1083).
//!
//! # What the claim was, and what is actually true of this crate
//!
//! The claim was that this crate builds its own oxigraph `SparqlEvaluator`, so in a host where
//! `oxigraph/http-client` is unified on (`ikigai-cli`, through rudof) a `SERVICE <http://…>` would
//! be an outbound request with no `urn:cap:net:*` near it. **It does not build one**: there is no
//! `SparqlEvaluator`, `Store::query` or `Store::update` anywhere in `src/`, and no dependency on
//! oxigraph at all. Every query and update is authored here and issued through the kernel to
//! `ikigai-store`'s doors (`src/sparql.rs`, `StoreClient`), with caller text entering only as
//! terms the store's own constructors build — so no ledger input can spell a `SERVICE` clause.
//!
//! ★ **But the composition WAS exposed, and through a door this crate makes every caller hold.**
//! A ledger reader must also hold `urn:cap:store:read:graph:urn:iki:ledger:graph:{name}` (a
//! sub-request carries the caller's capability unchanged, `README.md`, "What is enforced, and
//! where"), and a writer the matching write grant. With those, the caller can resolve
//! `urn:iki:store:graph-select` / `graph-update` directly, and up to `ikigai-store` 0.2.9 those
//! doors evaluated a caller's `SERVICE` with oxigraph's default HTTP handler. So the published
//! grant list for a ledger was a grant to make outbound requests. The fix is the store's
//! (0.2.10, `ikigai_store::service`); this crate's part is its floor, so no host composing this
//! ledger can resolve a store that still fetches.
//!
//! # How this file reproduces it
//!
//! With this crate's `http-client` feature (CI's `features: "*"`), which enables exactly
//! `ikigai-store/http-client` → `oxigraph/http-client`: the switch the hosts get by unification.
//! The CONTROL (raw oxigraph reaches the stub) is cfg'd on it — the sound direction, since the
//! feature on implies the handler is installed. The refusals are cfg'd on nothing, because a host
//! reaching the feature through rudof never enables this crate's. Every request goes to a stub on
//! 127.0.0.1 with an ephemeral port, never a real host.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use ikigai_core::{Capability, Error, Verb};

use common::*;

/// The ledger every test runs in: a named one, so the grants name one graph.
const LEDGER: &str = "acme";
/// That ledger's graph, spelled literally (see `tests/common`, "Spelled out, never computed").
const GRAPH: &str = "urn:iki:ledger:graph:acme";

/// The grant list `README.md` publishes for one ledger, as `tests/grants.rs` spells it.
fn published_grants() -> Capability {
    Capability::scoped([
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:write:{LEDGER}"),
        format!("urn:cap:ledger:delete:{LEDGER}"),
        format!("urn:cap:ledger:purge:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
        graveyard_write(LEDGER),
        graveyard_read(LEDGER),
    ])
}

// ----------------------------------------------------------------------------- the stub

/// A plain-HTTP stub that records the request line of every connection it is sent, and answers
/// each with an empty SPARQL result set (so a client that does reach it finishes).
struct Stub {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
}

/// The first bytes of the connection [`Stub::requests`] makes itself. Accepts are served in
/// backlog order, so once the stub has answered this one, every connection made before it has
/// been recorded: the negative assertions need no sleep and cannot race.
const SENTINEL: &[u8] = b"SENTINEL\r\n";

impl Stub {
    fn start() -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let head = read_head(&mut stream);
                if head.as_bytes().starts_with(SENTINEL) {
                    let _ = stream.write_all(b"ok");
                    continue;
                }
                record
                    .lock()
                    .unwrap()
                    .push(head.lines().next().unwrap_or("").to_string());
                let body = r#"{"head":{"vars":["s"]},"results":{"bindings":[]}}"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/sparql-results+json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Stub { addr, seen }
    }

    fn url(&self) -> String {
        format!("http://{}/sparql", self.addr)
    }

    /// Every request line the stub has been sent, after a sentinel round trip.
    fn requests(&self) -> Vec<String> {
        let mut sentinel = TcpStream::connect(self.addr).unwrap();
        sentinel.write_all(SENTINEL).unwrap();
        let mut ack = Vec::new();
        sentinel.read_to_end(&mut ack).unwrap();
        assert_eq!(ack, b"ok", "the stub did not answer its sentinel");
        self.seen.lock().unwrap().clone()
    }
}

/// Read up to the end of the request headers (or the sentinel line), enough to log it.
fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).map(|n| n == 1).unwrap_or(false) {
        head.push(byte[0]);
        if head == SENTINEL || head.len() > 64 * 1024 {
            break;
        }
        if head.ends_with(b"\r\n\r\n") {
            // Drain a body too, or closing with it unread resets the client's connection.
            let text = String::from_utf8_lossy(&head).to_ascii_lowercase();
            let length = text
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|n| n.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            let _ = stream.read_exact(&mut body);
            break;
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

// ----------------------------------------------------------- the doors a ledger hands out

/// ★ The reproduction. A caller holding exactly the published grant list for one ledger, and
/// nothing else, sends `SERVICE` through the two read doors that list opens. Refused as a typed
/// `InvalidArgument` naming `query`, and nothing reaches the stub. Up to `ikigai-store` 0.2.9, in
/// a build with `oxigraph/http-client` on, each of these was a request to the stub.
#[test]
fn a_ledger_reader_cannot_reach_the_network_through_its_graph_doors() {
    let stub = Stub::start();
    let kernel = kernel();
    let url = stub.url();
    let doors = [
        (
            "urn:iki:store:graph-select",
            format!("SELECT ?s WHERE {{ SERVICE <{url}> {{ ?s ?p ?o }} }}"),
        ),
        (
            "urn:iki:store:graph-select",
            format!("SELECT ?s WHERE {{ SERVICE SILENT <{url}> {{ ?s ?p ?o }} }}"),
        ),
        (
            "urn:iki:store:graph-ask",
            format!("ASK {{ SERVICE <{url}> {{ ?s ?p ?o }} }}"),
        ),
    ];
    let answers: Vec<_> = doors
        .iter()
        .map(|(door, query)| {
            let answer = try_as(
                &kernel,
                &published_grants(),
                Verb::Source,
                door,
                &[("graph", GRAPH), ("query", query)],
            );
            (door, query, answer)
        })
        .collect();
    // The egress first: it is the defect, and the typed refusal is how it is answered.
    assert_eq!(
        stub.requests(),
        Vec::<String>::new(),
        "a SERVICE clause reached the network under a ledger's published grants"
    );
    for (door, query, answer) in answers {
        let err = answer.expect_err(&format!("{door} answered {query}"));
        assert!(
            matches!(&err, Error::InvalidArgument { name, detail }
                if name == "query" && detail.contains("SERVICE")),
            "{door} {query}: not a typed SERVICE refusal: {err}"
        );
    }
}

/// The same for the write door the published list opens: `SERVICE` in an update's `WHERE`, and
/// `LOAD`, which no service handler governs (oxigraph builds `LOAD`'s client from the evaluator's
/// HTTP settings) and only the store's door check stops. Refused naming `content`; nothing sent.
#[test]
fn a_ledger_writer_cannot_reach_the_network_through_its_update_door() {
    let stub = Stub::start();
    let kernel = kernel();
    let url = stub.url();
    let updates = [
        format!(
            "INSERT {{ GRAPH <{GRAPH}> {{ <urn:s> <urn:p> ?o }} }} \
             WHERE {{ SERVICE <{url}> {{ ?s ?p ?o }} }}"
        ),
        format!("LOAD <{url}> INTO GRAPH <{GRAPH}>"),
        format!("LOAD SILENT <{url}> INTO GRAPH <{GRAPH}>"),
    ];
    let answers: Vec<_> = updates
        .iter()
        .map(|update| {
            let answer = try_as(
                &kernel,
                &published_grants(),
                Verb::Sink,
                "urn:iki:store:graph-update",
                &[("graph", GRAPH), ("content", update)],
            );
            (update, answer)
        })
        .collect();
    assert_eq!(
        stub.requests(),
        Vec::<String>::new(),
        "an update reached the network under a ledger's published grants"
    );
    for (update, answer) in answers {
        let err = answer.expect_err(&format!("graph-update applied {update}"));
        assert!(
            matches!(&err, Error::InvalidArgument { name, .. } if name == "content"),
            "{update}: not a typed refusal naming `content`: {err}"
        );
    }
}

// --------------------------------------------------------------- this crate's own doors

/// Ledger content that SPELLS a `SERVICE` or a `LOAD` is content: it is stored as text, read back
/// verbatim, and never becomes SPARQL. Exercised through filing, commenting, labeling, listing,
/// viewing and `next`, under the published grants; the stub sees nothing. (This holds in every
/// store version — the store builds the terms — and is pinned so it stays true.)
#[test]
fn ledger_content_that_spells_service_or_load_is_only_text() {
    let stub = Stub::start();
    let kernel = kernel();
    let grants = published_grants();
    let url = stub.url();
    let title = format!("}} }} SERVICE <{url}> {{ ?s ?p ?o }} LOAD <{url}>");
    let body =
        format!("\" }} ; LOAD <{url}> ; SELECT * WHERE {{ SERVICE <{url}> {{ ?s ?p ?o }} }}");
    let content = format!("{title}\n\n{body}");
    let filed = try_as(
        &kernel,
        &grants,
        Verb::Sink,
        "urn:iki:ledger:acme:append",
        &[("content", &content)],
    )
    .expect("filing an item whose text spells SERVICE");
    assert!(filed.starts_with("acme#1 "), "{filed}");
    try_as(
        &kernel,
        &grants,
        Verb::Sink,
        "urn:iki:ledger:acme:comment",
        &[("item", "1"), ("content", &body)],
    )
    .expect("commenting with text that spells SERVICE");

    let item = try_as(
        &kernel,
        &grants,
        Verb::Source,
        "urn:iki:ledger:acme:item:1",
        &[],
    )
    .expect("reading the item back");
    assert!(item.contains(&format!("SERVICE <{url}>")), "{item}");
    let items = try_as(
        &kernel,
        &grants,
        Verb::Source,
        "urn:iki:ledger:acme:items",
        &[],
    )
    .expect("listing the ledger");
    assert!(items.contains("SERVICE"), "{items}");

    assert_eq!(
        stub.requests(),
        Vec::<String>::new(),
        "ledger content reached the network"
    );
}

// ------------------------------------------------------------------------ the control

/// ★ The control, and the reason the refusals above are not vacuous in this build: with
/// `oxigraph/http-client` on, an evaluator the store did NOT build — raw oxigraph — really does
/// send `SERVICE` to the stub. If this stops reaching it (upstream moved the default, the feature
/// stopped switching it), the tests above prove nothing in this build, and this fails to say so.
#[cfg(feature = "http-client")]
#[test]
fn control_raw_oxigraph_with_http_client_reaches_the_stub() {
    use oxigraph::sparql::{QueryResults, SparqlEvaluator};
    let stub = Stub::start();
    let store = oxigraph::store::Store::new().unwrap();
    let query = format!(
        "SELECT ?s WHERE {{ SERVICE <{}> {{ ?s ?p ?o }} }}",
        stub.url()
    );
    let results = SparqlEvaluator::new()
        .parse_query(&query)
        .unwrap()
        .on_store(&store)
        .execute()
        .unwrap();
    let QueryResults::Solutions(solutions) = results else {
        panic!("not a solution set")
    };
    for solution in solutions {
        solution.unwrap();
    }
    let seen = stub.requests();
    assert_eq!(
        seen.len(),
        1,
        "raw oxigraph did not reach the stub: {seen:?}"
    );
}
