//! Changed-key set (`INCREMENTAL_RENDER_PIPELINE.md` §3): every committed
//! dispatch or host patch publishes the ordered, deduplicated set of state
//! keys it wrote. Reserved `@` signals are not state keys. The order is
//! lexicographic, chosen so the set is byte-identical across runs.

use opurom_core::{Event, Patch, Runtime, Value};
use serde_json::json;

fn ir() -> String {
    json!({
        "ir": "0.1.0", "app": "ChangedKeys",
        "state": [
            {"name": "count", "type": "int", "init": 0},
            {"name": "name", "type": "string", "init": ""},
            {"name": "dark", "type": "bool", "init": false},
            {"name": "visible", "type": "bool", "init": false}
        ],
        "locale": {"active": "en", "supported": ["en", "fr"]},
        "pages": ["Home"],
        "tree": {"id": 0, "kind": "Column", "props": {}, "events": {}, "children": [
            {"id": 1, "kind": "Button", "props": {}, "children": [], "events": {"tap": {"actions": [
                {"op": "set", "target": "count", "value": 5},
                {"op": "set", "target": "name", "value": "Ada"},
                {"op": "add", "target": "count", "value": 2},
                {"op": "set", "target": "@locale", "value": "fr"},
                {"op": "push", "target": "Home"}
            ]}}},
            {"id": 2, "kind": "Button", "props": {}, "children": [], "events": {"tap": {"actions": [
                {"op": "set", "target": "@locale", "value": "fr"},
                {"op": "push", "target": "Home"}
            ]}}},
            {"id": 3, "kind": "Button", "props": {}, "children": [], "events": {"tap": {"actions": [
                {"op": "add", "target": "count", "value": 1},
                {"op": "add", "target": "count", "value": 1}
            ]}}},
            {"id": 4, "kind": "Button", "props": {}, "children": [], "events": {
                "tap": {"op": "set", "target": "dark", "value": true}
            }},
            {"id": 5, "kind": "Button", "props": {}, "when": "visible", "children": [], "events": {
                "tap": {"op": "add", "target": "count", "value": 1}
            }},
            {"id": 6, "kind": "Input", "props": {"textBuffer": true}, "children": [], "events": {}}
        ]}
    })
    .to_string()
}

#[test]
fn a_mixed_action_batch_collects_only_state_keys_in_sorted_order() {
    let mut runtime = Runtime::from_ir_json(&ir()).unwrap();
    assert!(
        runtime.last_changed_keys().is_empty(),
        "initial set is empty"
    );

    let patches = runtime.dispatch(Event::Tap { node_id: 1 }).unwrap();
    // Patch order is action order; the changed-key order is lexicographic.
    let patch_names: Vec<_> = patches.iter().map(|patch| patch.name.as_str()).collect();
    assert_eq!(patch_names, ["count", "name", "count", "@locale", "@nav"]);
    assert_eq!(runtime.last_changed_keys(), ["count", "name"]);

    // Values and version still follow the unchanged dispatch semantics.
    assert_eq!(runtime.state("count"), Some(&Value::Int(7)));
    assert_eq!(runtime.state("name"), Some(&Value::Str("Ada".to_owned())));
    assert_eq!(runtime.locale().unwrap().active, "fr");
    assert_eq!(runtime.version(), 1);
}

#[test]
fn repeated_writes_to_one_key_are_deduplicated() {
    let mut runtime = Runtime::from_ir_json(&ir()).unwrap();
    runtime.dispatch(Event::Tap { node_id: 3 }).unwrap();
    assert_eq!(runtime.state("count"), Some(&Value::Int(2)));
    assert_eq!(runtime.last_changed_keys(), ["count"]);
}

#[test]
fn a_reserved_only_event_produces_an_empty_set() {
    let mut runtime = Runtime::from_ir_json(&ir()).unwrap();
    runtime.dispatch(Event::Tap { node_id: 1 }).unwrap();
    runtime.dispatch(Event::Tap { node_id: 2 }).unwrap();
    // The second event wrote no state: the set is cleared, not left stale.
    assert!(runtime.last_changed_keys().is_empty());
    assert_eq!(runtime.version(), 2);
}

#[test]
fn no_op_and_text_input_dispatches_produce_empty_sets() {
    let mut runtime = Runtime::from_ir_json(&ir()).unwrap();
    runtime.dispatch(Event::Tap { node_id: 4 }).unwrap();
    assert_eq!(runtime.last_changed_keys(), ["dark"]);

    // A hidden node changes nothing, so the previous set is cleared.
    runtime.dispatch(Event::Tap { node_id: 5 }).unwrap();
    assert!(runtime.last_changed_keys().is_empty());
    assert_eq!(runtime.version(), 1);

    // Text buffer edits are node-named patches, not state keys.
    runtime
        .dispatch(Event::Input {
            node_id: 6,
            text: r#"{"op":"set_text","text":"typed"}"#.to_owned(),
        })
        .unwrap();
    assert!(runtime.last_changed_keys().is_empty());
    assert_eq!(runtime.version(), 2);
}

#[test]
fn host_patches_publish_their_changed_keys() {
    let mut runtime = Runtime::from_ir_json(&ir()).unwrap();
    runtime
        .apply_patch(&Patch {
            name: "name".to_owned(),
            value: Value::Str("Grace".to_owned()),
        })
        .unwrap();
    assert_eq!(runtime.last_changed_keys(), ["name"]);
    assert_eq!(runtime.state("name"), Some(&Value::Str("Grace".to_owned())));
    assert_eq!(runtime.version(), 1);

    // The reserved @locale patch clears the state-key set.
    runtime
        .apply_patch(&Patch {
            name: "@locale".to_owned(),
            value: Value::Str("fr".to_owned()),
        })
        .unwrap();
    assert!(runtime.last_changed_keys().is_empty());
    assert_eq!(runtime.locale().unwrap().active, "fr");
    assert_eq!(runtime.version(), 2);
}

#[test]
fn the_changed_key_order_is_deterministic_across_runs() {
    let mut a = Runtime::from_ir_json(&ir()).unwrap();
    let mut b = Runtime::from_ir_json(&ir()).unwrap();
    a.dispatch(Event::Tap { node_id: 1 }).unwrap();
    b.dispatch(Event::Tap { node_id: 1 }).unwrap();
    let keys: &[String] = a.last_changed_keys();
    assert_eq!(keys, b.last_changed_keys());
    assert_eq!(format!("{keys:?}"), format!("{:?}", b.last_changed_keys()));
    assert_eq!(keys, ["count", "name"]);
}
