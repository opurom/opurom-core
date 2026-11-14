use opurom_core::{CoreError, Event, Ir, Patch, Runtime, Value};
use serde_json::{json, Value as Json};

fn nav_ir(pages: Json, action: Json) -> String {
    json!({
        "ir": "0.1",
        "app": "Nav",
        "state": [],
        "pages": pages,
        "tree": {
            "id": 0,
            "kind": "Button",
            "props": {"label": "Go"},
            "children": [],
            "events": {"tap": action}
        },
        "theme": {}
    })
    .to_string()
}

#[test]
fn declared_pages_are_parsed_in_declaration_order() {
    let ir = Ir::parse(&nav_ir(
        json!(["Home", "Settings"]),
        json!({"op": "push", "target": "Settings"}),
    ))
    .unwrap();
    assert_eq!(ir.pages, vec!["Home", "Settings"]);
}

#[test]
fn a_push_dispatches_a_navigation_signal_without_mutating_state() {
    let ir = nav_ir(
        json!(["Home", "Settings"]),
        json!({"op": "push", "target": "Settings"}),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(
        patches,
        vec![Patch {
            name: "@nav".to_string(),
            value: Value::Str("Settings".to_string()),
        }]
    );
    assert_eq!(runtime.version(), 1);
    assert!(
        runtime.snapshot().is_empty(),
        "navigation leaves state intact"
    );
    // A second push is idempotent: still a signal, still no state mutation.
    runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(runtime.version(), 2);
    assert!(runtime.snapshot().is_empty());
}

#[test]
fn a_push_inside_an_action_list_runs_in_order() {
    let ir = nav_ir(
        json!(["Home", "Settings"]),
        json!({"actions": [
            {"op": "push", "target": "Settings"},
            {"op": "push", "target": "Home"}
        ]}),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(patches.len(), 2);
    assert_eq!(patches[0].value, Value::Str("Settings".to_string()));
    assert_eq!(patches[1].value, Value::Str("Home".to_string()));
}

#[test]
fn a_push_to_an_undeclared_page_is_rejected() {
    let ir = nav_ir(json!(["Home"]), json!({"op": "push", "target": "Ghost"}));
    let err = Ir::parse(&ir).unwrap_err();
    assert!(matches!(err, CoreError::InvalidIr(_)), "{err:?}");
    assert!(err.to_string().contains("not a declared page"), "{err}");
}

#[test]
fn a_push_with_a_value_is_rejected() {
    let ir = nav_ir(
        json!(["Settings"]),
        json!({"op": "push", "target": "Settings", "value": 1}),
    );
    let err = Ir::parse(&ir).unwrap_err();
    assert!(
        err.to_string().contains("push must not have a value"),
        "{err}"
    );
}

#[test]
fn duplicate_pages_are_rejected() {
    let ir = nav_ir(
        json!(["Home", "Home"]),
        json!({"op": "push", "target": "Home"}),
    );
    let err = Ir::parse(&ir).unwrap_err();
    assert!(err.to_string().contains("duplicates"), "{err}");
}

#[test]
fn pages_must_be_an_array_of_strings() {
    let ir = nav_ir(json!(["Home", 7]), json!({"op": "push", "target": "Home"}));
    let err = Ir::parse(&ir).unwrap_err();
    assert!(err.to_string().contains("must be a string"), "{err}");
    let scalar = nav_ir(json!("Home"), json!({"op": "push", "target": "Home"}));
    assert!(Ir::parse(&scalar).is_err());
}

#[test]
fn ir_without_pages_keeps_its_existing_state_machine() {
    // No `pages` field: the kernel must behave exactly as before, including
    // the pinned state/patch stream.
    let ir = json!({
        "ir": "0.1",
        "app": "Counter",
        "state": [{"name": "count", "type": "int", "init": 0}],
        "tree": {
            "id": 0,
            "kind": "Button",
            "props": {},
            "children": [],
            "events": {"tap": {"op": "add", "target": "count", "value": 1}}
        },
        "theme": {}
    })
    .to_string();
    let parsed = Ir::parse(&ir).unwrap();
    assert!(parsed.pages.is_empty());
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(patches[0].name, "count");
    assert_eq!(patches[0].value, Value::Int(1));
    assert_eq!(runtime.state("count"), Some(&Value::Int(1)));
}

#[test]
fn a_push_without_any_declared_pages_is_rejected() {
    let ir = json!({
        "ir": "0.1",
        "app": "Nav",
        "state": [],
        "tree": {
            "id": 0,
            "kind": "Button",
            "props": {},
            "children": [],
            "events": {"tap": {"op": "push", "target": "Settings"}}
        },
        "theme": {}
    })
    .to_string();
    assert!(Ir::parse(&ir).is_err());
}
