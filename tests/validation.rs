use opurom_core::{Event, Ir, Patch, Runtime, Value};
use serde_json::{json, Value as Json};

fn validate_ir(action: Json) -> String {
    json!({
        "ir": "0.1",
        "app": "FormDemo",
        "state": [{"name": "name", "type": "string", "init": ""}],
        "tree": {
            "id": 0,
            "kind": "Button",
            "props": {"label": "Check"},
            "children": [],
            "events": {"tap": action}
        },
        "theme": {}
    })
    .to_string()
}

#[test]
fn a_validate_dispatches_a_reserved_signal_without_mutating_state() {
    let ir = validate_ir(json!({"op": "validate", "target": "form"}));
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(
        patches,
        vec![Patch {
            name: "@validate".to_string(),
            value: Value::Str("form".to_string()),
        }]
    );
    assert_eq!(runtime.state("name"), Some(&Value::Str(String::new())));
    assert_eq!(runtime.version(), 1);
}

#[test]
fn a_validate_inside_an_action_list_runs_in_order() {
    let ir = validate_ir(json!({"actions": [
        {"op": "validate", "target": "form"},
        {"op": "set", "target": "name", "value": "ok"}
    ]}));
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(patches.len(), 2);
    assert_eq!(patches[0].name, "@validate");
    assert_eq!(patches[0].value, Value::Str("form".to_string()));
    assert_eq!(patches[1].name, "name");
    assert_eq!(patches[1].value, Value::Str("ok".to_string()));
    assert_eq!(runtime.state("name"), Some(&Value::Str("ok".to_string())));
}

#[test]
fn a_validate_with_a_value_is_rejected() {
    let ir = validate_ir(json!({"op": "validate", "target": "form", "value": 1}));
    let err = Ir::parse(&ir).unwrap_err();
    assert!(
        err.to_string().contains("validate must not have a value"),
        "{err}"
    );
}

#[test]
fn a_validate_does_not_require_a_declared_state_or_pages() {
    // The form scope is a runtime consumer name, not a state or page.
    let ir = validate_ir(json!({"op": "validate", "target": "checkout"}));
    let parsed = Ir::parse(&ir).unwrap();
    assert!(parsed.pages.is_empty());
    let patches = Runtime::from_ir_json(&ir)
        .unwrap()
        .dispatch(Event::Tap { node_id: 0 })
        .unwrap();
    assert_eq!(patches[0].value, Value::Str("checkout".to_string()));
}
