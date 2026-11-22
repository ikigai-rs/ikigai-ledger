//! Research proof: two independently clocked hosts sharing one store.
mod common;

use common::*;
use ikigai_core::{Clock, Fallback, Kernel, Space, Time};
use ikigai_store::DurableStore;
use std::sync::Arc;

struct Fixed(u64);
impl Clock for Fixed {
    fn now(&self) -> Time {
        Time::from_millis(self.0)
    }
}
fn host(store: DurableStore, time: u64) -> Kernel {
    let spaces = Fallback::new(vec![
        Arc::new(ikigai_store::space(store)) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(spaces), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(Fixed(time)))
}
#[test]
fn later_comment_from_slow_host_is_presented_first() {
    let store = DurableStore::in_memory().unwrap();
    let fast = host(store.clone(), 1_789_430_460_000);
    let slow = host(store, 1_789_430_400_000);
    append(&fast, "clock skew proof", &[]);
    sink(
        &fast,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", "first write")],
    );
    sink(
        &slow,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", "second write")],
    );
    let detail = source(&fast, "urn:iki:ledger:item:1", &[]);
    assert!(
        detail.find("second write").unwrap() < detail.find("first write").unwrap(),
        "{detail}"
    );
}
