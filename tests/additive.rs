//! The kernel validates the state/event part of IR v0.1 and preserves every
//! additive field for downstream consumers. A program compiled by the language
//! layer must load even when it uses every newer shape at once; a future strict
//! check on an unknown field would otherwise break the whole example gallery.

use opurom_core::{Event, Runtime, Value};
use serde_json::{json, Value as Json};

fn additive_ir() -> Json {
    json!({
        "ir": "0.1",
        "app": "Additive",
        "pages": ["Home", "Settings"],
        "state": [
            {"name": "name", "type": "string", "init": ""},
            {"name": "level", "type": "int", "init": 0},
            {"name": "dark", "type": "bool", "init": false}
        ],
        "theme": {"mode": "system", "accent": "#097FEB", "radius": 12, "transition": "slide"},
        "tree": {
            "id": 0,
            "kind": "Column",
            "props": {"gap": 8, "opacity": 0.95, "maxLines": 2, "align": "center"},
            "children": [
                {
                    "id": 1,
                    "kind": "Column",
                    "props": {},
                    // `repeat.list` names a list state the language layer owns;
                    // the kernel preserves the field without resolving it.
                    "repeat": {"item": "entry", "list": "items"},
                    "component": "RowTemplate",
                    "children": [
                        {"id": 2, "kind": "Text", "props": {"value": "{entry}"},
                         "children": [], "events": {}}
                    ],
                    "events": {}
                },
                {
                    "id": 3,
                    "kind": "Image",
                    "props": {"src": "assets/photo.png", "fit": "cover", "width": 64, "height": 64},
                    "children": [], "events": {}
                },
                {
                    "id": 4,
                    "kind": "Input",
                    "props": {"bind": "name", "required": true, "minLength": 3,
                              "maxLength": 12, "pattern": "^[a-z]+$"},
                    "children": [],
                    "events": {"input": {"op": "set", "target": "name"}}
                },
                {
                    "id": 5,
                    "kind": "Slider",
                    "props": {"bind": "level"}, "children": [],
                    "events": {"input": {"op": "set", "target": "level"}}
                },
                {
                    "id": 6,
                    "kind": "Switch",
                    "props": {"bind": "dark"}, "children": [],
                    "events": {"input": {"op": "set", "target": "dark"}}
                },
                {
                    "id": 7,
                    "kind": "Button",
                    "props": {"label": "Go", "variant": "primary", "tone": "info", "duration": 240},
                    "children": [],
                    "events": {"tap": {"actions": [
                        {"op": "push", "target": "Settings"},
                        {"op": "validate", "target": "form"}
                    ]}}
                },
                {
                    "id": 8,
                    "kind": "Grid",
                    "props": {"columns": 3, "compactColumns": 2},
                    "children": [], "events": {}
                }
            ],
            "events": {}
        }
    })
}

#[test]
fn every_additive_ir_shape_loads_and_dispatches() {
    let ir = additive_ir().to_string();
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    assert_eq!(runtime.state("name"), Some(&Value::Str(String::new())));
    assert_eq!(runtime.state("level"), Some(&Value::Int(0)));
    assert_eq!(runtime.state("dark"), Some(&Value::Bool(false)));

    // Typed input sets still parse by declared type.
    runtime
        .dispatch(Event::Input {
            node_id: 5,
            text: "42".to_string(),
        })
        .unwrap();
    assert_eq!(runtime.state("level"), Some(&Value::Int(42)));

    // A mixed action list emits both reserved signals without mutating state.
    let patches = runtime.dispatch(Event::Tap { node_id: 7 }).unwrap();
    assert_eq!(patches.len(), 2);
    assert_eq!(patches[0].name, "@nav");
    assert_eq!(patches[0].value, Value::Str("Settings".to_string()));
    assert_eq!(patches[1].name, "@validate");
    assert_eq!(patches[1].value, Value::Str("form".to_string()));
    assert_eq!(runtime.state("level"), Some(&Value::Int(42)));
    assert_eq!(runtime.state("name"), Some(&Value::Str(String::new())));
}

#[test]
fn unknown_node_fields_stay_forward_compatible() {
    // A field the kernel does not know is preserved, never rejected: new
    // language features must not require a kernel change to load their IR.
    let mut ir = additive_ir();
    ir["tree"]["futureField"] = json!({"any": [1, "two", true]});
    assert!(Runtime::from_ir_json(&ir.to_string()).is_ok());
}
