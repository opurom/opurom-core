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

fn expected_node_patches(id: u32, display: &str, cursor: i64, start: i64, end: i64) -> Vec<Patch> {
    vec![
        Patch {
            name: format!("@textInput.{id}.display"),
            value: Value::Str(display.to_owned()),
        },
        Patch {
            name: format!("@textInput.{id}.cursor"),
            value: Value::Int(cursor),
        },
        Patch {
            name: format!("@textInput.{id}.selectionStart"),
            value: Value::Int(start),
        },
        Patch {
            name: format!("@textInput.{id}.selectionEnd"),
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
    assert_eq!(patches, expected_node_patches(1, "•••", 3, -1, -1));
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
    assert_eq!(patches, expected_node_patches(1, "••••", 4, -1, -1));
    assert_eq!(runtime.version(), 2);

    assert!(matches!(
        runtime.dispatch(Event::Tap { node_id: 1 }),
        Err(CoreError::UnboundEvent { node_id: 1, .. })
    ));
    assert_eq!(runtime.version(), 2);
}

fn form_runtime() -> Runtime {
    let ir = json!({
        "ir": "0.1.0",
        "app": "Form",
        "state": [],
        "theme": {},
        "tree": {
            "id": 0,
            "kind": "Column",
            "props": {},
            "children": [
                {
                    "id": 10,
                    "kind": "Input",
                    "props": {"textBuffer": true, "capacity": 32, "value": ""},
                    "children": [],
                    "events": {}
                },
                {
                    "id": 11,
                    "kind": "Input",
                    "props": {"textBuffer": true, "capacity": 32, "secure": true, "value": ""},
                    "children": [],
                    "events": {}
                },
                {
                    "id": 12,
                    "kind": "Input",
                    "props": {"textBuffer": true, "capacity": 32, "secure": true, "value": ""},
                    "children": [],
                    "events": {}
                },
                {
                    "id": 13,
                    "kind": "Text",
                    "props": {"value": "submit"},
                    "children": [],
                    "events": {}
                }
            ],
            "events": {}
        }
    })
    .to_string();
    Runtime::from_ir_json(&ir).unwrap()
}

fn input(runtime: &mut Runtime, node_id: u32, event: Json) -> Result<Vec<Patch>, CoreError> {
    runtime.dispatch(Event::Input {
        node_id,
        text: event_json(event),
    })
}

#[test]
fn three_input_form_routes_each_field_to_its_own_buffer() {
    let mut runtime = form_runtime();
    assert_eq!(
        input(&mut runtime, 10, json!({"op": "set_text", "text": "alice"})).unwrap(),
        expected_node_patches(10, "alice", 5, -1, -1)
    );
    assert_eq!(
        input(
            &mut runtime,
            11,
            json!({"op": "set_text", "text": "s3cret"})
        )
        .unwrap(),
        expected_node_patches(11, "••••••", 6, -1, -1)
    );
    assert_eq!(
        input(
            &mut runtime,
            12,
            json!({"op": "set_text", "text": "p@ss🔑"})
        )
        .unwrap(),
        expected_node_patches(12, "•••••", 5, -1, -1)
    );
    assert_eq!(runtime.version(), 3);

    let all = [
        input(&mut runtime, 10, json!({"op": "insert", "text": "!"})).unwrap(),
        input(&mut runtime, 11, json!({"op": "move_cursor", "index": 2})).unwrap(),
        input(&mut runtime, 12, json!({"op": "insert", "text": "Z"})).unwrap(),
    ];
    assert_eq!(all[0], expected_node_patches(10, "alice!", 6, -1, -1));
    assert_eq!(all[1], expected_node_patches(11, "••••••", 2, -1, -1));
    assert_eq!(all[2], expected_node_patches(12, "••••••", 6, -1, -1));
}

#[test]
fn editing_one_buffer_never_changes_the_others() {
    let mut runtime = form_runtime();
    input(&mut runtime, 10, json!({"op": "set_text", "text": "first"})).unwrap();
    input(
        &mut runtime,
        11,
        json!({"op": "set_text", "text": "second"}),
    )
    .unwrap();
    input(&mut runtime, 12, json!({"op": "set_text", "text": "third"})).unwrap();
    input(
        &mut runtime,
        11,
        json!({"op": "select", "start": 1, "end": 4}),
    )
    .unwrap();

    // A destructive edit on buffer 10 leaves 11's content and 12's content intact.
    input(&mut runtime, 10, json!({"op": "set_text", "text": "x"})).unwrap();
    assert_eq!(
        input(&mut runtime, 11, json!({"op": "insert", "text": "Y"})).unwrap(),
        expected_node_patches(11, "••••", 2, -1, -1)
    );
    assert_eq!(
        input(&mut runtime, 12, json!({"op": "move_cursor", "index": 5})).unwrap(),
        expected_node_patches(12, "•••••", 5, -1, -1)
    );

    // Secure masking stays per-node: unmasking 11 must not expose 12's raw text.
    input(
        &mut runtime,
        11,
        json!({"op": "set_secure", "secure": false}),
    )
    .unwrap();
    let revealed = input(&mut runtime, 11, json!({"op": "move_cursor", "index": 0})).unwrap();
    assert_eq!(revealed, expected_node_patches(11, "sYnd", 0, -1, -1));
    let still_masked = input(&mut runtime, 12, json!({"op": "move_cursor", "index": 0})).unwrap();
    assert_eq!(still_masked, expected_node_patches(12, "•••••", 0, -1, -1));
    let bytes = String::from_utf8(patch_bytes(&still_masked)).unwrap();
    assert!(!bytes.contains("third"));
}

#[test]
fn secure_fields_never_leak_raw_bytes_in_patches_or_errors() {
    let mut runtime = form_runtime();
    let secret_a = "hunter2😀";
    let secret_b = "correct horse";
    let set_a = input(
        &mut runtime,
        11,
        json!({"op": "set_text", "text": secret_a}),
    )
    .unwrap();
    let set_b = input(
        &mut runtime,
        12,
        json!({"op": "set_text", "text": secret_b}),
    )
    .unwrap();
    for (patches, secret) in [(&set_a, secret_a), (&set_b, secret_b)] {
        let bytes = String::from_utf8(patch_bytes(patches)).unwrap();
        assert!(!bytes.contains(secret) && !bytes.contains("hunter") && !bytes.contains("horse"));
    }
    assert_eq!(set_a, expected_node_patches(11, "••••••••", 8, -1, -1));
    assert_eq!(
        set_b,
        expected_node_patches(12, "•••••••••••••", 13, -1, -1)
    );

    // The bus response for an oversized secure edit is filtered through the
    // same per-node path and never echoes the raw text.
    let state = TextInputState::with_text("pw", 4, true).unwrap();
    let response = state
        .bus()
        .call(
            TEXT_INPUT_BUS,
            &event_json(json!({"op": "insert", "text": "RAW-SECRET"})),
        )
        .unwrap();
    assert!(!response.contains("RAW-SECRET"));
    assert!(matches!(
        state.dispatch_for(
            11,
            &event_json(json!({"op": "insert", "text": "RAW-SECRET"}))
        ),
        Err(TextInputError::Buffer(TextBufferError::CapacityExceeded {
            len: 2,
            added: 10,
            capacity: 4
        }))
    ));
}

#[test]
fn dispatch_to_unknown_or_non_buffer_nodes_is_a_typed_error_and_isolated() {
    let mut runtime = form_runtime();
    input(&mut runtime, 10, json!({"op": "set_text", "text": "keep"})).unwrap();
    let version = runtime.version();

    // An unknown node id is UnknownNode, nothing changes.
    assert!(matches!(
        runtime.dispatch_to(
            99,
            Event::Input {
                node_id: 99,
                text: event_json(json!({"op": "insert", "text": "nope"}))
            }
        ),
        Err(CoreError::UnknownNode(99))
    ));
    // A real but bufferless node is NoTextBufferInput, nothing changes.
    assert!(matches!(
        runtime.dispatch_to(
            13,
            Event::Input {
                node_id: 13,
                text: event_json(json!({"op": "insert", "text": "nope"}))
            }
        ),
        Err(CoreError::NoTextBufferInput(13))
    ));
    assert_eq!(runtime.version(), version);

    // Buffer 10 is intact and a valid edit on a different node still succeeds.
    assert_eq!(
        runtime
            .dispatch_to(
                10,
                Event::Input {
                    node_id: 10,
                    text: event_json(json!({"op": "insert", "text": "!"}))
                }
            )
            .unwrap(),
        expected_node_patches(10, "keep!", 5, -1, -1)
    );
    assert_eq!(
        input(&mut runtime, 12, json!({"op": "set_text", "text": "ok"})).unwrap(),
        expected_node_patches(12, "••", 2, -1, -1)
    );
}

#[test]
fn a_bad_edit_on_one_field_leaves_every_buffer_and_dispatch_intact() {
    let mut runtime = form_runtime();
    input(&mut runtime, 10, json!({"op": "set_text", "text": "aaa"})).unwrap();
    input(&mut runtime, 11, json!({"op": "set_text", "text": "bbb"})).unwrap();
    let version = runtime.version();

    let error = input(&mut runtime, 11, json!({"op": "move_cursor", "index": 99})).unwrap_err();
    assert!(matches!(
        error,
        CoreError::TextInput(TextInputError::Buffer(TextBufferError::CursorOutOfRange {
            index: 99,
            len: 3
        }))
    ));
    assert_eq!(runtime.version(), version);

    // Other buffers are untouched, and the next valid edit on a different node works.
    assert_eq!(
        input(&mut runtime, 10, json!({"op": "move_cursor", "index": 3})).unwrap(),
        expected_node_patches(10, "aaa", 3, -1, -1)
    );
    assert_eq!(
        input(&mut runtime, 12, json!({"op": "set_text", "text": "zz"})).unwrap(),
        expected_node_patches(12, "••", 2, -1, -1)
    );
}

#[test]
fn same_form_sequence_is_byte_identical_across_runs() {
    fn run() -> Vec<Vec<u8>> {
        let mut runtime = form_runtime();
        [
            (10, json!({"op": "set_text", "text": "name😀"})),
            (11, json!({"op": "set_text", "text": "pw"})),
            (12, json!({"op": "set_text", "text": "ab"})),
            (12, json!({"op": "select", "start": 0, "end": 2})),
            (11, json!({"op": "insert", "text": "é"})),
            (12, json!({"op": "replace_selection", "text": "🔑"})),
            (10, json!({"op": "backspace"})),
        ]
        .into_iter()
        .map(|(id, event)| patch_bytes(&input(&mut runtime, id, event).unwrap()))
        .collect()
    }
    let first = run();
    let second = run();
    assert_eq!(first, second);
    let joined = first
        .iter()
        .map(|bytes| String::from_utf8(bytes.clone()).unwrap())
        .collect::<String>();
    // Every patch is named for its own node, so a form binds each field.
    assert!(joined.contains("@textInput.10.display") && joined.contains("@textInput.11.display"));
    assert!(joined.contains("@textInput.10.display") && joined.contains("@textInput.12.display"));
}

#[test]
fn dispatch_to_rejects_a_non_input_event() {
    let mut runtime = form_runtime();
    assert!(matches!(
        runtime.dispatch_to(10, Event::Tap { node_id: 10 }),
        Err(CoreError::InvalidIr(_))
    ));
}
