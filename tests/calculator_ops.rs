use opurom_core::Runtime;
use serde_json::json;

fn ir(state: i64, events: serde_json::Value) -> String {
    json!({
        "ir": "0.1.0",
        "app": "CalcOps",
        "state": [{"name": "n", "type": "int", "init": state}],
        "tree": {"id": 0, "kind": "Button", "props": {"label": "op"}, "children": [], "events": events},
        "theme": {"mode": "system", "accent": "#097FEB", "radius": 12}
    }).to_string()
}

#[test]
fn mul_div_and_backspace_apply_to_integer_state() {
    let cases = [
        (json!({"tap": {"op": "mul", "target": "n", "value": 4}}), 20),
        (json!({"tap": {"op": "div", "target": "n", "value": 4}}), 1),
        (json!({"tap": {"op": "backspace", "target": "n"}}), 0),
    ];
    for (events, expected) in cases {
        let mut rt = Runtime::from_ir_json(&ir(5, events.clone())).unwrap();
        rt.dispatch(opurom_core::Event::Tap { node_id: 0 }).unwrap();
        assert_eq!(
            rt.state("n"),
            Some(&opurom_core::Value::Int(expected)),
            "{events}"
        );
    }
}

#[test]
fn division_by_zero_is_a_typed_error_at_load() {
    let mut rt = Runtime::from_ir_json(&ir(
        4,
        json!({"tap": {"op": "div", "target": "n", "value": 0}}),
    ))
    .unwrap();
    let error = rt
        .dispatch(opurom_core::Event::Tap { node_id: 0 })
        .unwrap_err();
    assert!(error.to_string().contains("division by zero"), "{error}");
}
