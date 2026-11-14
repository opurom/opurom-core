use opurom_core::{
    CoreError, Event, Patch, Runtime, Scheduler, TextBufferError, TextInputError, TextInputState,
    Value, TEXT_INPUT_BUS,
};
use serde_json::{json, Value as Json};
use std::sync::Arc;

fn event_json(event: Json) -> String {
    serde_json::to_string(&event).unwrap()
}

fn dispatch(state: &TextInputState, event: Json) -> Vec<Patch> {
    state.dispatch_json(&event_json(event)).unwrap()
}

fn expected_patches(display: &str, cursor: i64, start: i64, end: i64) -> Vec<Patch> {
    vec![
        Patch {
            name: "@textInput.display".to_owned(),
            value: Value::Str(display.to_owned()),
        },
        Patch {
            name: "@textInput.cursor".to_owned(),
            value: Value::Int(cursor),
        },
        Patch {
            name: "@textInput.selectionStart".to_owned(),
            value: Value::Int(start),
        },
        Patch {
            name: "@textInput.selectionEnd".to_owned(),
            value: Value::Int(end),
        },
    ]
}

fn patch_bytes(patches: &[Patch]) -> Vec<u8> {
    serde_json::to_vec(patches).unwrap()
}

#[test]
fn every_edit_op_travels_through_the_plugin_bus() {
    let state = TextInputState::new(8);
    let before = state.bus().metrics().calls;

    let patches = state
        .dispatch(Event::Input {
            node_id: 1,
            text: event_json(json!({"op": "set_text", "text": "a😀\u{4E2D}z"})),
        })
        .unwrap();
    assert_eq!(patches, expected_patches("a😀\u{4E2D}z", 4, -1, -1));
    assert_eq!(
        dispatch(&state, json!({"op": "move_cursor", "index": 2})),
        expected_patches("a😀\u{4E2D}z", 2, -1, -1)
    );
    assert_eq!(
        dispatch(&state, json!({"op": "insert", "text": "é"})),
        expected_patches("a😀é\u{4E2D}z", 3, -1, -1)
    );
    assert_eq!(
        dispatch(&state, json!({"op": "select", "start": 1, "end": 3})),
        expected_patches("a😀é\u{4E2D}z", 3, 1, 3)
    );
    assert_eq!(
        dispatch(&state, json!({"op": "replace_selection", "text": "✨"})),
        expected_patches("a✨\u{4E2D}z", 2, -1, -1)
    );
    assert_eq!(
        dispatch(&state, json!({"op": "backspace"})),
        expected_patches("a\u{4E2D}z", 1, -1, -1)
    );
    assert_eq!(
        dispatch(&state, json!({"op": "delete"})),
        expected_patches("az", 1, -1, -1)
    );
    assert_eq!(
        dispatch(&state, json!({"op": "set_secure", "secure": true})),
        expected_patches("••", 1, -1, -1)
    );

    assert_eq!(state.bus().metrics().calls - before, 8);
    assert_eq!(state.bus().metrics().trips, 0);
    assert!(!state.bus().is_disabled(TEXT_INPUT_BUS));
}

#[test]
fn secure_patches_and_errors_never_contain_raw_text() {
    let state = TextInputState::with_secure(16, true);
    let secret = "p@ss😀\u{4E2D}";
    let patches = dispatch(&state, json!({"op": "set_text", "text": secret}));
    assert_eq!(patches, expected_patches("••••••", 6, -1, -1));

    let text = String::from_utf8(patch_bytes(&patches)).unwrap();
    assert!(!text.contains(secret));
    assert!(!text.contains("p@ss"));
    assert!(!text.contains("😀"));
    assert!(!text.contains("\u{4E2D}"));

    let oversized = json!({"op": "insert", "text": "EXTRA-SECRET"});
    let response = state
        .bus()
        .call(TEXT_INPUT_BUS, &event_json(oversized))
        .unwrap();
    assert!(!response.contains("EXTRA-SECRET"));
    assert!(matches!(
        TextInputState::patches_from_response(&response),
        Err(TextInputError::Buffer(TextBufferError::CapacityExceeded {
            len: 6,
            added: 12,
            capacity: 16,
        }))
    ));
    assert_eq!(state.bus().metrics().trips, 0);
    assert_eq!(
        dispatch(&state, json!({"op": "move_cursor", "index": 2})),
        expected_patches("••••••", 2, -1, -1)
    );
    assert_eq!(
        dispatch(&state, json!({"op": "set_secure", "secure": false})),
        expected_patches(secret, 2, -1, -1)
    );
}

#[test]
fn malformed_edits_are_isolated_and_the_scheduler_recovers() {
    let state = Arc::new(TextInputState::with_text("abcdef", 8, false).unwrap());
    let scheduler = Scheduler::new(1);
    let initial = dispatch(state.as_ref(), json!({"op": "move_cursor", "index": 2}));
    assert_eq!(initial, expected_patches("abcdef", 2, -1, -1));

    let bad_edits: Vec<(Json, TextInputError)> = vec![
        (
            json!({"op": "move_cursor", "index": 99}),
            TextInputError::Buffer(TextBufferError::CursorOutOfRange { index: 99, len: 6 }),
        ),
        (
            json!({"op": "select", "start": 5, "end": 2}),
            TextInputError::Buffer(TextBufferError::InvalidRange { start: 5, end: 2 }),
        ),
        (
            json!({"op": "insert", "text": "xyz"}),
            TextInputError::Buffer(TextBufferError::CapacityExceeded {
                len: 6,
                added: 3,
                capacity: 8,
            }),
        ),
    ];
    for (event, expected) in bad_edits {
        let task_state = Arc::clone(&state);
        let handle = scheduler
            .spawn(1, move || task_state.dispatch_json(&event_json(event)))
            .unwrap();
        assert_eq!(handle.join().unwrap(), Err(expected));
    }
    assert_eq!(state.bus().metrics().trips, 0);
    assert!(!state.bus().is_disabled(TEXT_INPUT_BUS));
    assert_eq!(
        dispatch(state.as_ref(), json!({"op": "move_cursor", "index": 2})),
        initial
    );

    let recovery_state = Arc::clone(&state);
    let recovered = scheduler
        .spawn(1, move || {
            recovery_state.dispatch_json(&event_json(json!({"op": "insert", "text": "X"})))
        })
        .unwrap();
    assert_eq!(
        recovered.join().unwrap().unwrap(),
        expected_patches("abXcdef", 3, -1, -1)
    );
    scheduler.shutdown();
}

#[test]
fn same_event_sequence_produces_identical_patch_bytes() {
    fn sequence() -> Vec<Vec<u8>> {
        let state = TextInputState::with_secure(8, true);
        [
            json!({"op": "set_text", "text": "A😀\u{4E2D}"}),
            json!({"op": "select", "start": 1, "end": 3}),
            json!({"op": "replace_selection", "text": "✨\u{754C}"}),
            json!({"op": "move_cursor", "index": 2}),
            json!({"op": "backspace"}),
            json!({"op": "delete"}),
        ]
        .into_iter()
        .map(|event| patch_bytes(&dispatch(&state, event)))
        .collect()
    }

    let first = sequence();
    let second = sequence();
    assert_eq!(first, second);
    let joined = first
        .iter()
        .map(|bytes| String::from_utf8(bytes.clone()).unwrap())
        .collect::<String>();
    assert!(!joined.contains("A😀\u{4E2D}"));
    assert!(!joined.contains("✨\u{754C}"));
    assert_eq!(
        String::from_utf8(first.last().unwrap().clone()).unwrap(),
        r#"[{"name":"@textInput.display","value":"•"},{"name":"@textInput.cursor","value":1},{"name":"@textInput.selectionStart","value":-1},{"name":"@textInput.selectionEnd","value":-1}]"#
    );
}

#[test]
fn unicode_scalar_indices_are_integer_positions() {
    let state = TextInputState::new(6);
    assert_eq!(
        dispatch(&state, json!({"op": "set_text", "text": "a😀\u{4E2D}"})),
        expected_patches("a😀\u{4E2D}", 3, -1, -1)
    );
    let selected = dispatch(&state, json!({"op": "select", "start": 1, "end": 3}));
    assert_eq!(selected, expected_patches("a😀\u{4E2D}", 3, 1, 3));
    assert!(matches!(selected[1].value, Value::Int(3)));
    assert!(matches!(selected[2].value, Value::Int(1)));
    assert!(matches!(selected[3].value, Value::Int(3)));
    assert_eq!(
        dispatch(&state, json!({"op": "backspace"})),
        expected_patches("a", 1, -1, -1)
    );
    assert_eq!(
        dispatch(&state, json!({"op": "insert", "text": "é✨\u{754C}"})),
        expected_patches("aé✨\u{754C}", 4, -1, -1)
    );
    assert_eq!(
        dispatch(&state, json!({"op": "move_cursor", "index": 2})),
        expected_patches("aé✨\u{754C}", 2, -1, -1)
    );
    assert_eq!(
        dispatch(&state, json!({"op": "delete"})),
        expected_patches("aé\u{754C}", 2, -1, -1)
    );
}

fn runtime_with_text_input() -> Runtime {
    let ir = json!({
        "ir": "0.1.0",
        "app": "TextInput",
        "state": [],
        "theme": {},
        "tree": {
            "id": 0,
            "kind": "Column",
            "props": {},
            "children": [{
                "id": 1,
                "kind": "Input",
                "props": {"textBuffer": true, "capacity": 8, "secure": true, "value": ""},
                "children": [],
                "events": {}
            }],
            "events": {}
        }
    })
    .to_string();
    Runtime::from_ir_json(&ir).unwrap()
}

#[test]
fn runtime_owns_text_input_and_other_events_stay_unchanged() {
    let mut runtime = runtime_with_text_input();
    let patches = runtime
        .dispatch(Event::Input {
            node_id: 1,
            text: event_json(json!({"op": "set_text", "text": "pw😀"})),
        })
        .unwrap();
    assert_eq!(patches, expected_patches("•••", 3, -1, -1));
    assert_eq!(runtime.version(), 1);

    let error = runtime
        .dispatch(Event::Input {
            node_id: 1,
            text: event_json(json!({"op": "move_cursor", "index": 99})),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        CoreError::TextInput(TextInputError::Buffer(TextBufferError::CursorOutOfRange {
            index: 99,
            len: 3
        }))
    ));
    assert_eq!(runtime.version(), 1);

    let patches = runtime
        .dispatch(Event::Input {
            node_id: 1,
            text: event_json(json!({"op": "insert", "text": "x"})),
        })
        .unwrap();
    assert_eq!(patches, expected_patches("••••", 4, -1, -1));
    assert_eq!(runtime.version(), 2);

    assert!(matches!(
        runtime.dispatch(Event::Tap { node_id: 1 }),
        Err(CoreError::UnboundEvent { node_id: 1, .. })
    ));
    assert_eq!(runtime.version(), 2);
}
