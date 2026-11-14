use opurom_core::{
    CoreError, Event, Ir, Patch, PluginError, PluginHost, PluginStatus, ResourceError,
    ResourceManager, Runtime, Value,
};
use serde_json::{json, Value as Json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
const STUB_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/plugins/stub");

fn ir_with_tree(state: Json, tree: Json) -> String {
    json!({
        "ir": "0.1",
        "app": "T",
        "state": state,
        "tree": tree,
        "theme": {}
    })
    .to_string()
}

fn ir_document(state: Json, events: Json) -> String {
    ir_with_tree(
        state,
        json!({
            "id": 0,
            "kind": "Column",
            "props": {},
            "children": [],
            "events": events
        }),
    )
}

fn patch(name: &str, value: Value) -> Patch {
    Patch {
        name: name.to_owned(),
        value,
    }
}

/// Copies the stub plugin into a temporary directory so a test can set its
/// behaviour schedule without touching the checked-in fixture.
fn stub_plugin(tag: &str, schedule: &str) -> PathBuf {
    stub_plugin_named(tag, "stub", schedule)
}

fn stub_plugin_named(tag: &str, name: &str, schedule: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("opurom-core-stub-{tag}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for file in ["plugin.json", "stub.sh"] {
        fs::copy(Path::new(STUB_DIR).join(file), dir.join(file)).unwrap();
    }
    write_manifest(&dir, |manifest| manifest["name"] = json!(name));
    fs::write(dir.join("schedule"), format!("{schedule}\n")).unwrap();
    set_executable(&dir.join("stub.sh"));
    dir
}

fn set_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn host_with(allowlist: &[&str]) -> PluginHost {
    PluginHost::new(allowlist.iter().copied())
}

fn write_manifest(dir: &Path, edit: impl FnOnce(&mut Json)) {
    let path = dir.join("plugin.json");
    let text = fs::read_to_string(&path).unwrap();
    let mut manifest: Json = serde_json::from_str(&text).unwrap();
    edit(&mut manifest);
    fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
}

// --- multi-action events --------------------------------------------------

#[test]
fn multi_action_event_emits_ordered_patches() {
    let ir = ir_document(
        json!([{"name": "count", "type": "int", "init": 0}]),
        json!({
            "tap": {"actions": [
                {"op": "add", "target": "count", "value": 1},
                {"op": "add", "target": "count", "value": 10},
                {"op": "set", "target": "count", "value": 99}
            ]}
        }),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(
        patches,
        vec![
            patch("count", Value::Int(1)),
            patch("count", Value::Int(11)),
            patch("count", Value::Int(99)),
        ]
    );
    assert_eq!(runtime.state("count"), Some(&Value::Int(99)));
    assert_eq!(runtime.version(), 1);
}

#[test]
fn multi_action_event_is_atomic_when_a_later_action_fails() {
    let ir = ir_document(
        json!([
            {"name": "count", "type": "int", "init": 0},
            {"name": "max", "type": "int", "init": 9223372036854775807i64}
        ]),
        json!({
            "tap": {"actions": [
                {"op": "add", "target": "count", "value": 1},
                {"op": "add", "target": "max", "value": 1}
            ]}
        }),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let error = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap_err();
    assert!(error.to_string().contains("overflow"), "{error}");
    // The first action succeeded, but the whole event is rolled back.
    assert_eq!(runtime.state("count"), Some(&Value::Int(0)));
    assert_eq!(runtime.state("max"), Some(&Value::Int(i64::MAX)));
    assert_eq!(runtime.version(), 0);
}

#[test]
fn multi_action_event_is_atomic_when_a_later_action_overflows() {
    let ir = ir_document(
        json!([{"name": "max", "type": "int", "init": 9223372036854775807i64}]),
        json!({
            "tap": {"actions": [
                {"op": "add", "target": "max", "value": 1},
                {"op": "set", "target": "max", "value": 0}
            ]}
        }),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let error = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap_err();
    assert!(error.to_string().contains("overflow"), "{error}");
    assert_eq!(runtime.state("max"), Some(&Value::Int(i64::MAX)));
    assert_eq!(runtime.version(), 0);
}

#[test]
fn empty_actions_array_is_a_no_op_handler() {
    let ir = ir_document(
        json!([{"name": "count", "type": "int", "init": 3}]),
        json!({"tap": {"actions": []}}),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert!(patches.is_empty());
    assert_eq!(runtime.state("count"), Some(&Value::Int(3)));
    assert_eq!(runtime.version(), 0);
}

#[test]
fn value_from_reads_the_current_state_at_execution_time() {
    let ir = ir_document(
        json!([
            {"name": "a", "type": "int", "init": 0},
            {"name": "b", "type": "int", "init": 7}
        ]),
        json!({
            "tap": {"actions": [
                {"op": "set", "target": "a", "valueFrom": "b"},
                {"op": "add", "target": "a", "valueFrom": "b"}
            ]}
        }),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(
        patches,
        vec![patch("a", Value::Int(7)), patch("a", Value::Int(14))]
    );
    assert_eq!(runtime.state("a"), Some(&Value::Int(14)));
}

#[test]
fn sub_action_subtracts_and_add_adds_state_values() {
    // 10 - 3 = 7 for "sub"; 10 + 3 = 13 for "add".
    for (op, expected) in [("sub", 7), ("add", 13)] {
        let ir = ir_document(
            json!([
                {"name": "total", "type": "int", "init": 10},
                {"name": "step", "type": "int", "init": 3}
            ]),
            json!({"tap": {"op": op, "target": "total", "valueFrom": "step"}}),
        );
        let mut runtime = Runtime::from_ir_json(&ir).unwrap();
        let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
        assert_eq!(patches, vec![patch("total", Value::Int(expected))], "{op}");
        assert_eq!(runtime.state("total"), Some(&Value::Int(expected)), "{op}");
    }

    // A literal "sub" value is subtracted as well.
    let ir = ir_document(
        json!([{"name": "total", "type": "int", "init": 10}]),
        json!({"tap": {"op": "sub", "target": "total", "value": 4}}),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 0 }).unwrap(),
        vec![patch("total", Value::Int(6))]
    );
}

#[test]
fn sub_action_is_validated_like_add() {
    let int_state = json!([{"name": "total", "type": "int", "init": 10}]);
    let cases = [
        (
            json!([{"name": "total", "type": "bool", "init": false}]),
            json!({"tap": {"op": "sub", "target": "total", "value": 1}}),
            "sub requires an int target",
        ),
        (
            int_state,
            json!({"tap": {"op": "sub", "target": "total"}}),
            "sub requires an integer value",
        ),
    ];
    for (state, events, expected) in cases {
        let err = Runtime::from_ir_json(&ir_document(state, events)).unwrap_err();
        assert!(
            err.to_string().contains(expected),
            "expected {expected:?}, got {err}"
        );
    }
}

#[test]
fn value_from_sees_values_changed_by_earlier_actions() {
    let ir = ir_document(
        json!([
            {"name": "a", "type": "int", "init": 1},
            {"name": "b", "type": "int", "init": 0}
        ]),
        json!({
            "tap": {"actions": [
                {"op": "add", "target": "b", "value": 4},
                {"op": "add", "target": "a", "valueFrom": "b"}
            ]}
        }),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(
        patches,
        vec![patch("b", Value::Int(4)), patch("a", Value::Int(5))]
    );
}

#[test]
fn value_from_supports_strings_and_bools() {
    let ir = ir_document(
        json!([
            {"name": "a", "type": "string", "init": ""},
            {"name": "b", "type": "string", "init": "hello"},
            {"name": "flag", "type": "bool", "init": false},
            {"name": "other", "type": "bool", "init": true}
        ]),
        json!({
            "tap": {"actions": [
                {"op": "set", "target": "a", "valueFrom": "b"},
                {"op": "set", "target": "flag", "valueFrom": "other"}
            ]}
        }),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(
        patches,
        vec![
            patch("a", Value::Str("hello".to_owned())),
            patch("flag", Value::Bool(true)),
        ]
    );
}

#[test]
fn value_from_rejects_type_mismatches() {
    let cases: Vec<(Json, Json, &str)> = vec![
        (
            json!([
                {"name": "a", "type": "int", "init": 0},
                {"name": "b", "type": "string", "init": "x"}
            ]),
            json!({"tap": {"op": "set", "target": "a", "valueFrom": "b"}}),
            "same declared type",
        ),
        (
            json!([
                {"name": "a", "type": "int", "init": 0},
                {"name": "b", "type": "string", "init": "x"}
            ]),
            json!({"tap": {"op": "add", "target": "a", "valueFrom": "b"}}),
            "int valueFrom source",
        ),
        (
            json!([{"name": "a", "type": "int", "init": 0}]),
            json!({"tap": {"op": "set", "target": "a", "valueFrom": "missing"}}),
            "not declared in state",
        ),
        (
            json!([{"name": "a", "type": "int", "init": 0}]),
            json!({"tap": {"op": "set", "target": "a", "value": 1, "valueFrom": "a"}}),
            "mutually exclusive",
        ),
    ];
    for (state, events, expected) in cases {
        let error = Runtime::from_ir_json(&ir_document(state, events)).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?}, got {error}"
        );
    }
}

#[test]
fn toggle_is_unchanged_and_rejects_value_from() {
    let ir = ir_document(
        json!([
            {"name": "flag", "type": "bool", "init": false},
            {"name": "other", "type": "bool", "init": true}
        ]),
        json!({"tap": {"op": "toggle", "target": "flag"}}),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 0 }).unwrap(),
        vec![patch("flag", Value::Bool(true))]
    );

    let bad = ir_document(
        json!([{"name": "flag", "type": "bool", "init": false}]),
        json!({"tap": {"op": "toggle", "target": "flag", "valueFrom": "flag"}}),
    );
    assert!(Runtime::from_ir_json(&bad)
        .unwrap_err()
        .to_string()
        .contains("must not have a value"));
}

#[test]
fn single_action_events_still_work() {
    let ir = ir_document(
        json!([{"name": "count", "type": "int", "init": 0}]),
        json!({"tap": {"op": "add", "target": "count", "value": 2}}),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 0 }).unwrap(),
        vec![patch("count", Value::Int(2))]
    );
}

#[test]
fn node_when_field_is_preserved_in_the_parsed_ir() {
    let ir = json!({
        "ir": "0.1",
        "app": "T",
        "state": [{"name": "visible", "type": "bool", "init": true}],
        "tree": {
            "id": 0, "kind": "Column", "props": {},
            "when": "visible",
            "children": [{"id": 1, "kind": "Text", "props": {}, "children": [], "events": {}}],
            "events": {}
        }
    })
    .to_string();
    let parsed = Ir::parse(&ir).unwrap();
    assert_eq!(parsed.tree.when.as_deref(), Some("visible"));
    assert_eq!(parsed.tree.children[0].when, None);
}

#[test]
fn a_hidden_node_dispatch_is_inert_until_its_condition_turns_true() {
    let ir = ir_with_tree(
        json!([
            {"name": "visible", "type": "bool", "init": false},
            {"name": "count", "type": "int", "init": 0},
            {"name": "label", "type": "string", "init": "idle"}
        ]),
        json!({
            "id": 0, "kind": "Column", "props": {}, "events": {},
            "children": [
                {"id": 1, "kind": "Switch", "props": {}, "children": [],
                 "events": {"tap": {"op": "toggle", "target": "visible"}}},
                {"id": 2, "kind": "Button", "props": {}, "children": [],
                 "when": "visible",
                 "events": {
                     "tap": {"op": "add", "target": "count", "value": 1},
                     "input": {"op": "set", "target": "label"}
                 }}
            ]
        }),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    assert!(runtime
        .dispatch(Event::Tap { node_id: 2 })
        .unwrap()
        .is_empty());
    assert!(runtime
        .dispatch(Event::Input {
            node_id: 2,
            text: "typed".to_owned(),
        })
        .unwrap()
        .is_empty());
    assert_eq!(runtime.version(), 0);
    assert_eq!(runtime.state("count"), Some(&Value::Int(0)));
    assert_eq!(runtime.state("label"), Some(&Value::Str("idle".to_owned())));
    assert_eq!(runtime.snapshot()[1].value, Value::Int(0));

    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 1 }).unwrap(),
        vec![patch("visible", Value::Bool(true))]
    );
    assert_eq!(runtime.version(), 1);
    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 2 }).unwrap(),
        vec![patch("count", Value::Int(1))]
    );
    assert_eq!(
        runtime
            .dispatch(Event::Input {
                node_id: 2,
                text: "typed".to_owned(),
            })
            .unwrap(),
        vec![patch("label", Value::Str("typed".to_owned()))]
    );
    assert_eq!(runtime.version(), 3);

    runtime.dispatch(Event::Tap { node_id: 1 }).unwrap();
    let version = runtime.version();
    assert!(runtime
        .dispatch(Event::Tap { node_id: 2 })
        .unwrap()
        .is_empty());
    assert_eq!(runtime.version(), version);
    assert_eq!(runtime.state("count"), Some(&Value::Int(1)));
}

#[test]
fn an_ancestor_condition_covers_the_whole_subtree() {
    let ir = ir_with_tree(
        json!([
            {"name": "outer", "type": "bool", "init": false},
            {"name": "inner", "type": "bool", "init": true},
            {"name": "count", "type": "int", "init": 0}
        ]),
        json!({
            "id": 0, "kind": "Column", "props": {}, "events": {},
            "children": [
                {"id": 1, "kind": "Container", "props": {}, "when": "outer", "events": {},
                 "children": [
                     {"id": 2, "kind": "Button", "props": {}, "children": [],
                      "events": {"tap": {"op": "add", "target": "count", "value": 1}}},
                     {"id": 3, "kind": "Button", "props": {}, "children": [],
                      "when": "inner",
                      "events": {"tap": {"op": "add", "target": "count", "value": 10}}}
                 ]},
                {"id": 4, "kind": "Switch", "props": {}, "children": [],
                 "events": {"tap": {"op": "toggle", "target": "outer"}}},
                {"id": 5, "kind": "Switch", "props": {}, "children": [],
                 "events": {"tap": {"op": "toggle", "target": "inner"}}}
            ]
        }),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    assert!(runtime
        .dispatch(Event::Tap { node_id: 2 })
        .unwrap()
        .is_empty());
    assert!(runtime
        .dispatch(Event::Tap { node_id: 3 })
        .unwrap()
        .is_empty());

    runtime.dispatch(Event::Tap { node_id: 4 }).unwrap();
    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 2 }).unwrap(),
        vec![patch("count", Value::Int(1))]
    );
    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 3 }).unwrap(),
        vec![patch("count", Value::Int(11))]
    );

    runtime.dispatch(Event::Tap { node_id: 5 }).unwrap();
    assert!(runtime
        .dispatch(Event::Tap { node_id: 3 })
        .unwrap()
        .is_empty());
    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 2 }).unwrap(),
        vec![patch("count", Value::Int(12))]
    );

    runtime.dispatch(Event::Tap { node_id: 4 }).unwrap();
    assert!(runtime
        .dispatch(Event::Tap { node_id: 2 })
        .unwrap()
        .is_empty());
    assert_eq!(runtime.state("count"), Some(&Value::Int(12)));
}

#[test]
fn a_when_condition_must_name_a_declared_bool_state() {
    let cases = [
        (
            json!([{"name": "count", "type": "int", "init": 0}]),
            json!("count"),
            "must name a bool state",
        ),
        (
            json!([{"name": "visible", "type": "bool", "init": true}]),
            json!("missing"),
            "is not declared in state",
        ),
    ];
    for (state, when, expected) in cases {
        let ir = ir_with_tree(
            state,
            json!({"id": 0, "kind": "Column", "props": {}, "children": [],
                   "when": when, "events": {}}),
        );
        let error = Runtime::from_ir_json(&ir).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    }

    let non_string = ir_with_tree(
        json!([{"name": "visible", "type": "bool", "init": true}]),
        json!({"id": 0, "kind": "Column", "props": {}, "children": [],
               "when": 7, "events": {}}),
    );
    let error = Runtime::from_ir_json(&non_string).unwrap_err().to_string();
    assert!(error.contains("when must be a string"), "{error}");
}

#[test]
fn a_hidden_node_still_reports_unknown_and_unbound_events() {
    let ir = ir_with_tree(
        json!([{"name": "visible", "type": "bool", "init": false}]),
        json!({
            "id": 0, "kind": "Column", "props": {}, "events": {},
            "when": "visible",
            "children": [{"id": 1, "kind": "Button", "props": {}, "children": [],
                          "events": {"tap": {"op": "toggle", "target": "visible"}}}]
        }),
    );
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    assert!(matches!(
        runtime.dispatch(Event::Tap { node_id: 99 }),
        Err(CoreError::UnknownNode(99))
    ));
    assert!(matches!(
        runtime.dispatch(Event::Input {
            node_id: 1,
            text: "text".to_owned(),
        }),
        Err(CoreError::UnboundEvent { node_id: 1, .. })
    ));
    assert!(runtime
        .dispatch(Event::Tap { node_id: 1 })
        .unwrap()
        .is_empty());
    assert_eq!(runtime.version(), 0);
}

#[test]
fn malformed_ir_is_rejected_with_a_readable_diagnostic() {
    let cases: Vec<(String, &str)> = vec![
        (
            ir_document(
                json!([{"name": "a", "type": "int", "init": 0}]),
                json!({"tap": {"actions": {"op": "set", "target": "a", "value": 1}}}),
            ),
            "actions must be an array",
        ),
        (
            ir_document(
                json!([{"name": "a", "type": "int", "init": 0}]),
                json!({"tap": {"actions": [], "op": "set", "target": "a", "value": 1}}),
            ),
            "must not also contain op",
        ),
        (
            ir_document(
                json!([{"name": "a", "type": "int", "init": 0}]),
                json!({"tap": {"actions": [{"op": "set", "target": "a", "value": "x"}]}}),
            ),
            "expects int",
        ),
        (
            ir_document(
                json!([{"name": "a", "type": "int", "init": 0}]),
                json!({"tap": {"actions": [{"op": "mul", "target": "a", "value": 1}]}}),
            ),
            "not set, add, sub or toggle",
        ),
        (
            ir_document(
                json!([{"name": "a", "type": "int", "init": 0}]),
                json!({"tap": {"actions": [{"op": "add", "target": "a"}]}}),
            ),
            "add requires an integer value",
        ),
        ("{".to_owned(), "invalid IR JSON"),
    ];
    for (ir, expected) in cases {
        let error = Runtime::from_ir_json(&ir).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?}, got {error}"
        );
    }
}

// --- plugin host ---------------------------------------------------------

#[test]
fn plugin_handshake_and_successful_invoke() {
    let dir = stub_plugin("ok", "ok");
    let host = host_with(&["stub.echo"]);
    let info = host.load(&dir).unwrap();
    assert_eq!(info.name, "stub");
    assert_eq!(info.version, "0.1.0");
    assert_eq!(info.capabilities, vec!["stub.echo", "stub.fail"]);
    assert_eq!(info.status, PluginStatus::Active);
    assert_eq!(info.invocation_count, 0);

    let result = host.invoke("stub.echo", json!({"x": 1})).unwrap();
    assert_eq!(result["capability"], "stub.echo");

    let listed = host.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].invocation_count, 1);
    assert_eq!(listed[0].status, PluginStatus::Active);
}

#[test]
fn plugin_unknown_capability_is_an_error_value() {
    let dir = stub_plugin("unknown", "ok");
    let host = host_with(&["stub.echo", "stub.nope"]);
    host.load(&dir).unwrap();
    match host.invoke("stub.nope", json!({})) {
        Err(PluginError::UnknownCapability(name)) => assert_eq!(name, "stub.nope"),
        other => panic!("expected unknown capability, got {other:?}"),
    }
}

#[test]
fn plugin_reports_the_error_message_from_the_plugin() {
    let dir = stub_plugin("fail", "ok");
    let host = host_with(&["stub.fail"]);
    host.load(&dir).unwrap();
    match host.invoke("stub.fail", json!({})) {
        Err(PluginError::Failed(message)) => {
            assert!(message.contains("unknown capability"), "{message}")
        }
        other => panic!("expected plugin failure, got {other:?}"),
    }
}

#[test]
fn plugin_allowlist_denies_unlisted_capabilities() {
    let dir = stub_plugin("allowlist", "ok");
    let host = host_with(&["stub.echo"]);
    host.load(&dir).unwrap();
    match host.invoke("stub.fail", json!({})) {
        Err(PluginError::NotAllowed(name)) => assert_eq!(name, "stub.fail"),
        other => panic!("expected allowlist denial, got {other:?}"),
    }
}

#[test]
fn plugin_timeout_is_reported_and_bounded() {
    let dir = stub_plugin("timeout", "stall");
    let host = PluginHost::new(["stub.echo"]).with_timeout(Duration::from_millis(150));
    host.load(&dir).unwrap();
    let started = Instant::now();
    let error = host.invoke("stub.echo", json!({})).unwrap_err();
    let elapsed = started.elapsed();
    assert!(matches!(error, PluginError::Timeout(_)), "{error:?}");
    assert!(
        elapsed < Duration::from_secs(2),
        "timeout took {elapsed:?}, the host must not block forever"
    );
}

#[test]
fn three_consecutive_timeouts_disable_the_plugin() {
    let dir = stub_plugin("strike", "stall");
    let host = PluginHost::new(["stub.echo"]).with_timeout(Duration::from_millis(150));
    host.load(&dir).unwrap();
    for _ in 0..3 {
        assert!(matches!(
            host.invoke("stub.echo", json!({})),
            Err(PluginError::Timeout(_))
        ));
    }
    let listed = host.list().unwrap();
    assert_eq!(listed[0].status, PluginStatus::Disabled);
    match host.invoke("stub.echo", json!({})) {
        Err(PluginError::Disabled(name)) => assert_eq!(name, "stub"),
        other => panic!("expected disabled plugin, got {other:?}"),
    }
}

#[test]
fn a_success_resets_the_timeout_counter() {
    let dir = stub_plugin("reset", "stall,ok,stall,stall");
    let host = PluginHost::new(["stub.echo"]).with_timeout(Duration::from_millis(150));
    host.load(&dir).unwrap();
    assert!(matches!(
        host.invoke("stub.echo", json!({})),
        Err(PluginError::Timeout(_))
    ));
    // The success below resets the counter.
    assert!(host.invoke("stub.echo", json!({})).is_ok());
    assert_eq!(host.list().unwrap()[0].status, PluginStatus::Active);
    assert!(matches!(
        host.invoke("stub.echo", json!({})),
        Err(PluginError::Timeout(_))
    ));
    assert!(matches!(
        host.invoke("stub.echo", json!({})),
        Err(PluginError::Timeout(_))
    ));
    // Two timeouts after the reset are still below the threshold of three.
    assert_eq!(host.list().unwrap()[0].status, PluginStatus::Active);
}

#[test]
fn a_crash_is_never_cleared_by_a_later_success() {
    let dir = stub_plugin("crash-then-ok", "crash,ok");
    let host = PluginHost::new(["stub.echo"]).with_timeout(Duration::from_millis(200));
    host.load(&dir).unwrap();
    assert!(matches!(
        host.invoke("stub.echo", json!({})),
        Err(PluginError::Crashed(_))
    ));
    // The plugin answers again, but the crash already happened: the counter keeps growing.
    assert!(host.invoke("stub.echo", json!({})).is_ok());
    assert_eq!(host.list().unwrap()[0].status, PluginStatus::Active);
}

#[test]
fn three_crashes_disable_the_plugin() {
    let dir = stub_plugin("crashes", "crash");
    let host = PluginHost::new(["stub.echo"]).with_timeout(Duration::from_millis(200));
    host.load(&dir).unwrap();
    for _ in 0..3 {
        assert!(matches!(
            host.invoke("stub.echo", json!({})),
            Err(PluginError::Crashed(_))
        ));
    }
    assert_eq!(host.list().unwrap()[0].status, PluginStatus::Disabled);
}

#[test]
fn protocol_violations_are_detected() {
    let dir = stub_plugin("bad-id", "bad-id");
    let host = PluginHost::new(["stub.echo"]).with_timeout(Duration::from_millis(200));
    host.load(&dir).unwrap();
    match host.invoke("stub.echo", json!({})) {
        Err(PluginError::ProtocolViolation(message)) => {
            assert!(message.contains("does not match"), "{message}")
        }
        other => panic!("expected protocol violation, got {other:?}"),
    }

    let dir = stub_plugin("garbage", "garbage");
    let host = PluginHost::new(["stub.echo"]).with_timeout(Duration::from_millis(200));
    host.load(&dir).unwrap();
    match host.invoke("stub.echo", json!({})) {
        Err(PluginError::ProtocolViolation(message)) => {
            assert!(message.contains("malformed response JSON"), "{message}")
        }
        other => panic!("expected protocol violation, got {other:?}"),
    }
}

#[test]
fn advertised_capabilities_must_be_declared_in_the_manifest() {
    let dir = stub_plugin("liar", "ok");
    fs::write(dir.join("caps"), "stub.echo,stub.undeclared\n").unwrap();
    let host = host_with(&["stub.echo"]);
    match host.load(&dir) {
        Err(PluginError::ProtocolViolation(message)) => {
            assert!(message.contains("not declared"), "{message}")
        }
        other => panic!("expected protocol violation, got {other:?}"),
    }
}

#[test]
fn manifest_validation_rejects_bad_manifests() {
    let cases: Vec<(&str, &str)> = vec![
        ("name", "does not match"),
        ("version", "is not x.y.z"),
        ("protocol", "is not 1.0"),
        ("capabilities", "must not be empty"),
    ];
    for (field, expected) in cases {
        let broken = stub_plugin(&format!("manifest-{field}"), "ok");
        write_manifest(&broken, |manifest| match field {
            "name" => manifest["name"] = json!("Bad Name"),
            "version" => manifest["version"] = json!("1.0"),
            "protocol" => manifest["protocol"] = json!("2.0"),
            "capabilities" => manifest["capabilities"] = json!([]),
            _ => unreachable!(),
        });
        let host = host_with(&["stub.echo"]);
        match host.load(&broken) {
            Err(PluginError::InvalidManifest(message)) => {
                assert!(
                    message.contains(expected),
                    "expected {expected:?}, got {message}"
                )
            }
            other => panic!("expected manifest rejection for {field}, got {other:?}"),
        }
    }
}

#[test]
fn manifest_entry_traversal_is_rejected() {
    let dir = stub_plugin("traversal", "ok");
    write_manifest(&dir, |manifest| {
        manifest["entry"] = json!("../../etc/passwd")
    });
    let host = host_with(&["stub.echo"]);
    match host.load(&dir) {
        Err(PluginError::InvalidManifest(message)) => {
            assert!(message.contains("must stay inside"), "{message}")
        }
        other => panic!("expected traversal rejection, got {other:?}"),
    }
}

#[test]
fn manifest_entry_must_be_an_executable_file() {
    let dir = stub_plugin("not-executable", "ok");
    fs::write(dir.join("data.txt"), "not an executable").unwrap();
    write_manifest(&dir, |manifest| manifest["entry"] = json!("./data.txt"));
    let host = host_with(&["stub.echo"]);
    match host.load(&dir) {
        Err(PluginError::InvalidManifest(message)) => {
            assert!(message.contains("not an executable file"), "{message}")
        }
        other => panic!("expected executable check, got {other:?}"),
    }
}

#[test]
fn a_missing_plugin_directory_is_an_error_value() {
    let host = host_with(&["stub.echo"]);
    match host.load(Path::new(FIXTURES).join("does-not-exist")) {
        Err(PluginError::Io(message)) => assert!(message.contains("does-not-exist"), "{message}"),
        other => panic!("expected I/O error, got {other:?}"),
    }
}

#[test]
fn a_broken_plugin_does_not_stop_other_plugins() {
    let broken = stub_plugin_named("broken", "broken", "crash");
    let healthy = stub_plugin_named("healthy", "healthy", "ok");
    let host = host_with(&["stub.echo"]);
    host.load(&broken).unwrap();
    host.load(&healthy).unwrap();
    for _ in 0..3 {
        assert!(host.invoke_on("broken", "stub.echo", json!({})).is_err());
    }
    let listed = host.list().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].name, "broken");
    assert_eq!(listed[0].status, PluginStatus::Disabled);
    assert_eq!(listed[1].name, "healthy");
    assert_eq!(listed[1].status, PluginStatus::Active);
    // The healthy plugin keeps serving.
    assert!(host.invoke_on("healthy", "stub.echo", json!({})).is_ok());
    // The disabled plugin is skipped, so the capability routes to the healthy one.
    assert!(host.invoke("stub.echo", json!({})).is_ok());
}

// --- resource manager ----------------------------------------------------

#[test]
fn resource_cache_evicts_in_least_recently_used_order() {
    let mut cache: ResourceManager<String, Vec<u8>> = ResourceManager::new(100);
    cache.insert("a".to_owned(), vec![0; 40], 40).unwrap();
    cache.insert("b".to_owned(), vec![0; 40], 40).unwrap();
    assert!(cache.get(&"a".to_owned()).is_some());
    // "a" is now the most recent entry, so "b" is evicted first.
    cache.insert("c".to_owned(), vec![0; 40], 40).unwrap();
    assert!(cache.get(&"b".to_owned()).is_none());
    assert!(cache.get(&"a".to_owned()).is_some());
    assert!(cache.get(&"c".to_owned()).is_some());
    assert_eq!(cache.used_bytes(), 80);
}

#[test]
fn resource_cache_rejects_values_above_the_budget() {
    let mut cache: ResourceManager<String, Vec<u8>> = ResourceManager::new(10);
    match cache.insert("big".to_owned(), vec![0; 11], 11) {
        Err(ResourceError::BudgetExceeded { required, budget }) => {
            assert_eq!(required, 11);
            assert_eq!(budget, 10);
        }
        other => panic!("expected budget error, got {other:?}"),
    }
    assert!(cache.is_empty());
}

#[test]
fn resource_cache_handles_zero_sized_values() {
    let mut cache: ResourceManager<String, String> = ResourceManager::new(2);
    cache.insert("a".to_owned(), String::new(), 0).unwrap();
    cache.insert("b".to_owned(), String::new(), 0).unwrap();
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.used_bytes(), 2);
    cache.insert("c".to_owned(), String::new(), 0).unwrap();
    assert_eq!(cache.len(), 2);
    assert!(cache.get(&"a".to_owned()).is_none());
}

#[test]
fn resource_cache_replacing_a_key_keeps_the_budget() {
    let mut cache: ResourceManager<String, String> = ResourceManager::new(100);
    cache
        .insert("a".to_owned(), "first".to_owned(), 10)
        .unwrap();
    cache
        .insert("a".to_owned(), "second".to_owned(), 20)
        .unwrap();
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.used_bytes(), 20);
}

#[test]
fn runtime_plugin_hook_caches_results_within_the_budget() {
    let mut runtime = Runtime::from_ir_json(&ir_document(json!([]), json!({}))).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let result = runtime
        .invoke_plugin("stub.echo", json!({"a": 1}), move |_, _| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"ok": true}))
        })
        .unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // The second call is served from the cache.
    let counter = Arc::clone(&calls);
    runtime
        .invoke_plugin("stub.echo", json!({"a": 1}), move |_, _| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"ok": false}))
        })
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Different params miss the cache.
    let counter = Arc::clone(&calls);
    runtime
        .invoke_plugin("stub.echo", json!({"a": 2}), move |_, _| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"ok": true}))
        })
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(runtime.resource_cache().len(), 2);
}

#[test]
fn runtime_plugin_hook_reports_plugin_errors() {
    let mut runtime = Runtime::from_ir_json(&ir_document(json!([]), json!({}))).unwrap();
    let error = runtime
        .invoke_plugin("stub.echo", json!({}), |_, _| {
            Err(PluginError::NotAllowed("stub.echo".to_owned()))
        })
        .unwrap_err();
    assert!(matches!(
        error,
        CoreError::Plugin(PluginError::NotAllowed(_))
    ));
}

#[test]
fn runtime_records_network_requests_through_the_network_capability() {
    let mut runtime = Runtime::from_ir_json(&ir_document(json!([]), json!({}))).unwrap();
    assert_eq!(runtime.network_records(), json!([]));

    // A network.request is captured with its method, URL and reported status.
    runtime
        .invoke_plugin(
            "network.request",
            json!({"method": "GET", "url": "https://api.example.com/items"}),
            |_, _| Ok(json!({"status": 200, "body": {"items": []}})),
        )
        .unwrap();
    let records = runtime.network_records();
    let arr = records.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["method"], "GET");
    assert_eq!(arr[0]["url"], "https://api.example.com/items");
    assert_eq!(arr[0]["status"], 200);
    assert!(arr[0]["requestBytes"].as_u64().unwrap() > 0);
    assert!(arr[0]["responseBytes"].as_u64().unwrap() > 0);

    // A non-network capability is not captured.
    runtime
        .invoke_plugin("stub.echo", json!({"a": 1}), |_, _| Ok(json!({"ok": true})))
        .unwrap();
    assert_eq!(runtime.network_records().as_array().unwrap().len(), 1);
}

#[test]
fn runtime_plugin_hook_uses_a_real_plugin() {
    let dir = stub_plugin("runtime-hook", "ok");
    let host = host_with(&["stub.echo"]);
    host.load(&dir).unwrap();
    let mut runtime = Runtime::from_ir_json(&ir_document(json!([]), json!({}))).unwrap();
    let result = runtime
        .invoke_plugin("stub.echo", json!({}), |capability, params| {
            host.invoke(capability, params)
        })
        .unwrap();
    assert_eq!(result["capability"], "stub.echo");
}

// --- CLI -----------------------------------------------------------------

#[test]
fn cli_plugins_subcommand_invokes_a_capability() {
    let dir = stub_plugin("cli", "ok");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_opurom-core"))
        .arg("plugins")
        .arg(&dir)
        .arg("stub.echo")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let result: Json = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(result["capability"], "stub.echo");
}

#[test]
fn cli_plugins_subcommand_reports_errors_on_stderr() {
    let dir = stub_plugin("cli-error", "ok");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_opurom-core"))
        .arg("plugins")
        .arg(&dir)
        .arg("stub.not-allowed")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no loaded plugin declares"), "{stderr}");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_opurom-core"))
        .arg("plugins")
        .arg(FIXTURES)
        .arg("stub.echo")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cannot load plugin"), "{stderr}");
}

#[test]
fn cli_plugins_subcommand_needs_a_directory_and_a_capability() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_opurom-core"))
        .arg("plugins")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
