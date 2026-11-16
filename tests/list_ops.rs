//! Dynamic lists (DYNAMIC_LISTS Phase A): the kernel half of the three list
//! event ops, driven by the event shapes the compiler emits — `append`
//! (literal or `valueFrom`), `removeAt` by index and `assign` whole-list.
//! Reject paths cover non-list targets, element type mismatches and an index
//! past the live end of the list.

use opurom_core::{CoreError, Event, Runtime, Value};
use serde_json::{json, Value as Json};

/// One program with every list mutation binding plus scalar controls, so the
/// list ops are proven next to the untouched `set`/`add`/`toggle`/input path.
const IR: &str = r##"{
  "ir": "0.1.0", "app": "ListOps",
  "state": [
    {"name": "items", "type": "list", "init": ["seed", "next"]},
    {"name": "empty", "type": "list", "init": []},
    {"name": "backup", "type": "list", "init": ["old"]},
    {"name": "draft", "type": "string", "init": "later"},
    {"name": "count", "type": "int", "init": 0},
    {"name": "dark", "type": "bool", "init": false}
  ],
  "pages": ["Home"],
  "tree": {
    "id": 0, "kind": "Column", "props": {}, "events": {},
    "children": [
      {"id": 1, "kind": "Button", "props": {"label": "a"}, "children": [], "events": {
        "tap": {"op": "append", "target": "items", "value": "third"}}},
      {"id": 2, "kind": "Button", "props": {"label": "b"}, "children": [], "events": {
        "tap": {"op": "append", "target": "items", "valueFrom": "draft"}}},
      {"id": 3, "kind": "Button", "props": {"label": "c"}, "children": [], "events": {
        "tap": {"op": "append", "target": "empty", "value": "first"}}},
      {"id": 4, "kind": "Button", "props": {"label": "d"}, "children": [], "events": {
        "tap": {"op": "removeAt", "target": "items", "value": 0}}},
      {"id": 5, "kind": "Button", "props": {"label": "e"}, "children": [], "events": {
        "tap": {"op": "assign", "target": "items", "value": ["a", "b"]}}},
      {"id": 6, "kind": "Button", "props": {"label": "f"}, "children": [], "events": {
        "tap": {"op": "assign", "target": "items", "valueFrom": "backup"}}},
      {"id": 7, "kind": "Button", "props": {"label": "g"}, "children": [], "events": {
        "tap": {"op": "removeAt", "target": "items", "value": 5}}},
      {"id": 8, "kind": "Button", "props": {"label": "h"}, "children": [], "events": {
        "tap": {"actions": [
          {"op": "append", "target": "items", "value": "x"},
          {"op": "removeAt", "target": "items", "value": 99}
        ]}}},
      {"id": 9, "kind": "Button", "props": {"label": "i"}, "children": [], "events": {
        "tap": {"op": "add", "target": "count", "value": 1}}},
      {"id": 10, "kind": "Button", "props": {"label": "j"}, "children": [], "events": {
        "tap": {"op": "toggle", "target": "dark"}}},
      {"id": 11, "kind": "Button", "props": {"label": "k"}, "children": [], "events": {
        "tap": {"op": "set", "target": "draft", "value": "other"}}},
      {"id": 12, "kind": "Input", "props": {"bind": "count"}, "children": [], "events": {
        "input": {"op": "set", "target": "count"}}}
    ]
  }
}"##;

fn app() -> Runtime {
    Runtime::from_ir_json(IR).expect("fixture parses")
}

fn list(items: &[&str]) -> Value {
    Value::List(items.iter().map(|s| (*s).to_owned()).collect())
}

/// Parses a one-button program whose single tap action is the given JSON and
/// returns the rejection.
fn rejects(state: Json, action: Json) -> CoreError {
    let ir = json!({
        "ir": "0.1.0", "app": "Rejects",
        "state": state,
        "pages": ["Home"],
        "tree": {"id": 0, "kind": "Column", "props": {}, "events": {}, "children": [
            {"id": 1, "kind": "Button", "props": {"label": "go"}, "children": [],
             "events": {"tap": action}}
        ]}
    });
    Runtime::from_ir_json(&ir.to_string()).expect_err("IR must be rejected")
}

fn scalar_state() -> Json {
    json!([{"name": "count", "type": "int", "init": 0}])
}

fn list_state() -> Json {
    json!([{"name": "items", "type": "list", "init": ["seed"]}])
}

#[test]
fn append_grows_the_list_and_patches_the_whole_value() {
    let mut runtime = app();
    let patches = runtime.dispatch(Event::Tap { node_id: 1 }).unwrap();
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].name, "items");
    assert_eq!(patches[0].value, list(&["seed", "next", "third"]));
    // Consumers read a list patch as a plain JSON string array.
    assert_eq!(
        serde_json::to_value(&patches[0]).unwrap(),
        json!({"name": "items", "value": ["seed", "next", "third"]})
    );
    assert_eq!(
        runtime.state("items"),
        Some(&list(&["seed", "next", "third"]))
    );
    assert_eq!(runtime.version(), 1);
}

#[test]
fn append_reads_a_string_state_through_value_from() {
    let mut runtime = app();
    runtime.dispatch(Event::Tap { node_id: 2 }).unwrap();
    assert_eq!(
        runtime.state("items"),
        Some(&list(&["seed", "next", "later"]))
    );
    // The source state itself is untouched.
    assert_eq!(
        runtime.state("draft"),
        Some(&Value::Str("later".to_owned()))
    );
}

#[test]
fn append_fills_an_empty_list() {
    // An empty init declares string elements, so the list takes data as soon
    // as an event lands in it.
    let mut runtime = app();
    runtime.dispatch(Event::Tap { node_id: 3 }).unwrap();
    assert_eq!(runtime.state("empty"), Some(&list(&["first"])));
}

#[test]
fn remove_at_drops_the_indexed_element() {
    let mut runtime = app();
    let patches = runtime.dispatch(Event::Tap { node_id: 4 }).unwrap();
    assert_eq!(patches[0].value, list(&["next"]));
    assert_eq!(runtime.state("items"), Some(&list(&["next"])));
}

#[test]
fn assign_replaces_the_list_whole() {
    let mut runtime = app();
    runtime.dispatch(Event::Tap { node_id: 5 }).unwrap();
    assert_eq!(runtime.state("items"), Some(&list(&["a", "b"])));
    runtime.dispatch(Event::Tap { node_id: 6 }).unwrap();
    assert_eq!(runtime.state("items"), Some(&list(&["old"])));
}

#[test]
fn remove_at_past_the_end_errors_without_touching_state_or_version() {
    let mut runtime = app();
    let error = runtime.dispatch(Event::Tap { node_id: 7 }).unwrap_err();
    assert_eq!(
        error,
        CoreError::IndexOutOfBounds {
            name: "items".to_owned(),
            index: 5,
            len: 2,
        }
    );
    assert_eq!(runtime.state("items"), Some(&list(&["seed", "next"])));
    assert_eq!(runtime.version(), 0);
}

#[test]
fn a_failing_action_aborts_the_whole_event() {
    let mut runtime = app();
    let error = runtime.dispatch(Event::Tap { node_id: 8 }).unwrap_err();
    assert!(
        matches!(error, CoreError::IndexOutOfBounds { .. }),
        "{error:?}"
    );
    // The append that ran first is rolled back with the failing index.
    assert_eq!(runtime.state("items"), Some(&list(&["seed", "next"])));
    assert_eq!(runtime.version(), 0);
}

#[test]
fn scalar_ops_stay_unchanged_beside_the_list_ops() {
    let mut runtime = app();
    runtime.dispatch(Event::Tap { node_id: 9 }).unwrap();
    runtime.dispatch(Event::Tap { node_id: 10 }).unwrap();
    runtime.dispatch(Event::Tap { node_id: 11 }).unwrap();
    runtime
        .dispatch(Event::Input {
            node_id: 12,
            text: "7".to_owned(),
        })
        .unwrap();
    assert_eq!(runtime.state("count"), Some(&Value::Int(7)));
    assert_eq!(runtime.state("dark"), Some(&Value::Bool(true)));
    assert_eq!(
        runtime.state("draft"),
        Some(&Value::Str("other".to_owned()))
    );
    assert_eq!(runtime.state("items"), Some(&list(&["seed", "next"])));
}

#[test]
fn append_rejects_a_non_list_target() {
    let error = rejects(
        scalar_state(),
        json!({"op": "append", "target": "count", "value": "x"}),
    );
    assert!(
        matches!(&error, CoreError::InvalidIr(message) if message.contains("append requires a list target")),
        "{error:?}"
    );
}

#[test]
fn append_rejects_a_non_string_value() {
    let error = rejects(
        list_state(),
        json!({"op": "append", "target": "items", "value": 3}),
    );
    assert!(
        matches!(&error, CoreError::InvalidIr(message) if message.contains("append requires a string value")),
        "{error:?}"
    );
}

#[test]
fn append_rejects_a_value_from_that_is_not_a_string_state() {
    let error = rejects(
        json!([
            {"name": "items", "type": "list", "init": []},
            {"name": "count", "type": "int", "init": 0}
        ]),
        json!({"op": "append", "target": "items", "valueFrom": "count"}),
    );
    assert!(
        matches!(&error, CoreError::InvalidIr(message) if message.contains("must be a string state")),
        "{error:?}"
    );
}

#[test]
fn append_rejects_a_missing_value() {
    let error = rejects(list_state(), json!({"op": "append", "target": "items"}));
    assert!(
        matches!(&error, CoreError::InvalidIr(message) if message.contains("append requires a value")),
        "{error:?}"
    );
}

#[test]
fn remove_at_rejects_a_non_list_target_a_negative_index_and_a_value_from() {
    let on_scalar = rejects(
        scalar_state(),
        json!({"op": "removeAt", "target": "count", "value": 0}),
    );
    assert!(
        matches!(&on_scalar, CoreError::InvalidIr(message) if message.contains("removeAt requires a list target")),
        "{on_scalar:?}"
    );

    let negative = rejects(
        list_state(),
        json!({"op": "removeAt", "target": "items", "value": -1}),
    );
    assert!(
        matches!(&negative, CoreError::InvalidIr(message) if message.contains("non-negative integer")),
        "{negative:?}"
    );

    let from = rejects(
        list_state(),
        json!({"op": "removeAt", "target": "items", "valueFrom": "items"}),
    );
    assert!(
        matches!(&from, CoreError::InvalidIr(message) if message.contains("removeAt must not have a valueFrom")),
        "{from:?}"
    );
}

#[test]
fn assign_rejects_a_non_list_target_and_a_bad_array() {
    let on_scalar = rejects(
        scalar_state(),
        json!({"op": "assign", "target": "count", "value": ["a"]}),
    );
    assert!(
        matches!(&on_scalar, CoreError::InvalidIr(message) if message.contains("assign requires a list target")),
        "{on_scalar:?}"
    );

    let not_array = rejects(
        list_state(),
        json!({"op": "assign", "target": "items", "value": "a"}),
    );
    assert!(
        matches!(&not_array, CoreError::InvalidIr(message) if message.contains("value must be an array")),
        "{not_array:?}"
    );

    let bad_element = rejects(
        list_state(),
        json!({"op": "assign", "target": "items", "value": ["a", 2]}),
    );
    assert!(
        matches!(&bad_element, CoreError::InvalidIr(message) if message.contains("value[1] must be a string")),
        "{bad_element:?}"
    );

    let from_scalar = rejects(
        json!([
            {"name": "items", "type": "list", "init": []},
            {"name": "draft", "type": "string", "init": ""}
        ]),
        json!({"op": "assign", "target": "items", "valueFrom": "draft"}),
    );
    assert!(
        matches!(&from_scalar, CoreError::InvalidIr(message) if message.contains("must have the same declared type")),
        "{from_scalar:?}"
    );
}

#[test]
fn set_cannot_replace_a_list_use_assign() {
    let literal = rejects(
        list_state(),
        json!({"op": "set", "target": "items", "value": ["a"]}),
    );
    assert!(
        matches!(&literal, CoreError::InvalidIr(message) if message.contains("use assign")),
        "{literal:?}"
    );

    let from = rejects(
        json!([
            {"name": "items", "type": "list", "init": []},
            {"name": "backup", "type": "list", "init": []}
        ]),
        json!({"op": "set", "target": "items", "valueFrom": "backup"}),
    );
    assert!(
        matches!(&from, CoreError::InvalidIr(message) if message.contains("use assign")),
        "{from:?}"
    );
}

#[test]
fn a_list_state_requires_an_array_of_strings() {
    let not_array = rejects(
        json!([{"name": "items", "type": "list", "init": "seed"}]),
        json!({"op": "append", "target": "items", "value": "x"}),
    );
    assert!(
        matches!(&not_array, CoreError::InvalidIr(message) if message.contains("init must be an array")),
        "{not_array:?}"
    );

    let bad_element = rejects(
        json!([{"name": "items", "type": "list", "init": [1]}]),
        json!({"op": "append", "target": "items", "value": "x"}),
    );
    assert!(
        matches!(&bad_element, CoreError::InvalidIr(message) if message.contains("init[0] must be a string")),
        "{bad_element:?}"
    );
}
