use opurom_core::{
    AdapterFault, CoreError, Event, Host, Ir, Patch, PluginBus, PluginError, Runtime, Scheduler,
    SchedulerError, Value,
};
use serde_json::{json, Value as Json};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};

const GOLDEN: &str = include_str!("fixtures/counter.opuir.json");
const FORM: &str = include_str!("fixtures/form.opuir.json");

fn patch(name: &str, value: Value) -> Patch {
    Patch {
        name: name.to_owned(),
        value,
    }
}

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

fn run_cli(ir_path: &str, commands: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_opurom-core"))
        .arg(ir_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(commands.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn rejects_bad_ir_version() {
    let bad = GOLDEN.replace("\"ir\": \"0.1\"", "\"ir\": \"0.2\"");
    assert_ne!(bad, GOLDEN);
    let error = Runtime::from_ir_json(&bad).unwrap_err();
    assert!(error.to_string().contains("unsupported IR version"), "{error}");

    let missing = GOLDEN.replace("\"ir\": \"0.1\",", "");
    let error = Runtime::from_ir_json(&missing).unwrap_err();
    assert!(error.to_string().contains("ir"), "{error}");
}

#[test]
fn rejects_malformed_json_and_non_object_root() {
    assert!(matches!(
        Runtime::from_ir_json("{"),
        Err(CoreError::InvalidJson(_))
    ));
    assert!(matches!(
        Runtime::from_ir_json("[]"),
        Err(CoreError::InvalidIr(_))
    ));
    assert!(matches!(
        Runtime::from_ir_json("null"),
        Err(CoreError::InvalidIr(_))
    ));
}

#[test]
fn rejects_bad_state_types_and_duplicate_names() {
    let cases: Vec<(Json, &str)> = vec![
        (
            json!([{"name": "x", "type": "float", "init": 0}]),
            "int, bool or string",
        ),
        (
            json!([{"name": "x", "type": "int", "init": "0"}]),
            "integer for type int",
        ),
        (
            json!([{"name": "x", "type": "bool", "init": 1}]),
            "bool for type bool",
        ),
        (
            json!([{"name": "x", "type": "string", "init": 7}]),
            "string for type string",
        ),
        (
            json!([{"name": "x", "type": "int", "init": 1.5}]),
            "integer for type int",
        ),
        (json!([{"name": "x", "type": "int"}]), "missing"),
        (
            json!([
                {"name": "x", "type": "int", "init": 0},
                {"name": "x", "type": "bool", "init": true}
            ]),
            "duplicate state name",
        ),
    ];
    for (state, expected) in cases {
        let error = Runtime::from_ir_json(&ir_document(state, json!({}))).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?}, got {error}"
        );
    }
    let not_array = json!({
        "ir": "0.1", "app": "T", "state": {},
        "tree": {"id": 0, "kind": "Column", "props": {}, "children": [], "events": {}}
    })
    .to_string();
    assert!(Runtime::from_ir_json(&not_array)
        .unwrap_err()
        .to_string()
        .contains("root.state must be an array"));
}

#[test]
fn rejects_duplicate_node_ids_and_missing_node_fields() {
    let duplicated = ir_with_tree(
        json!([]),
        json!({
            "id": 0, "kind": "Column", "props": {}, "events": {},
            "children": [{"id": 0, "kind": "Text", "props": {}, "children": [], "events": {}}]
        }),
    );
    assert!(Runtime::from_ir_json(&duplicated)
        .unwrap_err()
        .to_string()
        .contains("duplicate node id"));

    let incomplete = ir_with_tree(json!([]), json!({"id": 0}));
    assert!(Runtime::from_ir_json(&incomplete)
        .unwrap_err()
        .to_string()
        .contains("missing"));
}

#[test]
fn rejects_illegal_event_actions() {
    let int_state = json!([{"name": "count", "type": "int", "init": 0}]);
    let bool_state = json!([{"name": "flag", "type": "bool", "init": false}]);
    let cases: Vec<(Json, Json, &str)> = vec![
        (
            bool_state.clone(),
            json!({"tap": {"op": "add", "target": "flag", "value": 1}}),
            "int target",
        ),
        (
            int_state.clone(),
            json!({"tap": {"op": "add", "target": "count"}}),
            "integer value",
        ),
        (
            int_state.clone(),
            json!({"tap": {"op": "toggle", "target": "count"}}),
            "bool target",
        ),
        (
            bool_state,
            json!({"tap": {"op": "toggle", "target": "flag", "value": true}}),
            "toggle must not have a value",
        ),
        (
            int_state.clone(),
            json!({"tap": {"op": "set", "target": "count", "value": "1"}}),
            "expects int",
        ),
        (
            int_state.clone(),
            json!({"tap": {"op": "set", "target": "count"}}),
            "only legal for input events",
        ),
        (
            int_state.clone(),
            json!({"input": {"op": "set", "target": "count"}}),
            "string target",
        ),
        (
            int_state.clone(),
            json!({"tap": {"op": "mul", "target": "count", "value": 2}}),
            "not set, add or toggle",
        ),
        (
            int_state.clone(),
            json!({"tap": {"op": "set", "target": "missing", "value": 1}}),
            "not declared in state",
        ),
        (
            int_state,
            json!({"tap": "not-an-action"}),
            "must be an object",
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
fn golden_ir_loads() {
    let ir = Ir::parse(GOLDEN).unwrap();
    assert_eq!(ir.version, "0.1");
    assert_eq!(ir.app, "Counter");
    assert_eq!(ir.state.len(), 1);
    assert_eq!(ir.tree.children.len(), 2);
    assert!(ir.theme.is_object());

    let runtime = Runtime::from_ir_json(GOLDEN).unwrap();
    assert_eq!(runtime.app(), "Counter");
    assert_eq!(runtime.version(), 0);
    assert_eq!(runtime.state("count"), Some(&Value::Int(0)));
    assert_eq!(runtime.state("absent"), None);
    assert_eq!(runtime.snapshot(), vec![patch("count", Value::Int(0))]);
}

#[test]
fn tap_plus_and_minus_produce_patches_and_bump_version() {
    let mut runtime = Runtime::from_ir_json(GOLDEN).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 4 }).unwrap();
    assert_eq!(patches, vec![patch("count", Value::Int(1))]);
    assert_eq!(runtime.version(), 1);
    assert_eq!(runtime.state("count"), Some(&Value::Int(1)));

    let patches = runtime.dispatch(Event::Tap { node_id: 3 }).unwrap();
    assert_eq!(patches, vec![patch("count", Value::Int(0))]);
    assert_eq!(runtime.version(), 2);
    assert_eq!(
        serde_json::to_string(&patches).unwrap(),
        r#"[{"name":"count","value":0}]"#
    );
}

#[test]
fn input_toggle_and_set_apply_typed_patches() {
    let mut runtime = Runtime::from_ir_json(FORM).unwrap();
    let patches = runtime
        .dispatch(Event::Input {
            node_id: 1,
            text: "hello world".to_owned(),
        })
        .unwrap();
    assert_eq!(
        patches,
        vec![patch("label", Value::Str("hello world".to_owned()))]
    );
    assert_eq!(runtime.version(), 1);

    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 2 }).unwrap(),
        vec![patch("flag", Value::Bool(true))]
    );
    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 2 }).unwrap(),
        vec![patch("flag", Value::Bool(false))]
    );
    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 3 }).unwrap(),
        vec![patch("n", Value::Int(9))]
    );
    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 5 }).unwrap(),
        vec![patch("n", Value::Int(11))]
    );
    assert_eq!(
        runtime.dispatch(Event::Tap { node_id: 4 }).unwrap(),
        vec![patch("label", Value::Str("bye".to_owned()))]
    );
    assert_eq!(runtime.version(), 6);
    assert_eq!(
        runtime.snapshot(),
        vec![
            patch("label", Value::Str("bye".to_owned())),
            patch("flag", Value::Bool(false)),
            patch("n", Value::Int(11)),
        ]
    );
}

#[test]
fn patches_serialize_as_json() {
    assert_eq!(
        serde_json::to_string(&patch("n", Value::Int(-3))).unwrap(),
        r#"{"name":"n","value":-3}"#
    );
    assert_eq!(
        serde_json::to_string(&patch("b", Value::Bool(true))).unwrap(),
        r#"{"name":"b","value":true}"#
    );
    assert_eq!(
        serde_json::to_string(&patch("s", Value::Str("x".to_owned()))).unwrap(),
        r#"{"name":"s","value":"x"}"#
    );
}

#[test]
fn dispatch_errors_and_overflow_leave_state_unchanged() {
    let mut runtime = Runtime::from_ir_json(GOLDEN).unwrap();
    assert!(matches!(
        runtime.dispatch(Event::Tap { node_id: 99 }),
        Err(CoreError::UnknownNode(99))
    ));
    assert!(matches!(
        runtime.dispatch(Event::Tap { node_id: 1 }),
        Err(CoreError::UnboundEvent { node_id: 1, .. })
    ));
    assert!(matches!(
        runtime.dispatch(Event::Input {
            node_id: 4,
            text: "x".to_owned()
        }),
        Err(CoreError::UnboundEvent { .. })
    ));
    assert_eq!(runtime.version(), 0);
    assert_eq!(runtime.state("count"), Some(&Value::Int(0)));

    let overflow_ir = ir_document(
        json!([{"name": "max", "type": "int", "init": 9223372036854775807i64}]),
        json!({"tap": {"op": "add", "target": "max", "value": 1}}),
    );
    let mut runtime = Runtime::from_ir_json(&overflow_ir).unwrap();
    let error = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap_err();
    assert!(error.to_string().contains("overflow"), "{error}");
    assert_eq!(runtime.version(), 0);
    assert_eq!(runtime.state("max"), Some(&Value::Int(i64::MAX)));
}

#[test]
fn plugin_panic_is_isolated_and_circuit_breaker_opens() {
    let bus = PluginBus::new(2);
    bus.register("echo", |input| Ok(format!("echo:{input}")));
    bus.register("boom", |_| -> Result<String, String> {
        panic!("plugin exploded")
    });

    assert_eq!(bus.call("echo", "x").unwrap(), "echo:x");
    assert!(matches!(
        bus.call("boom", "a"),
        Err(PluginError::Panicked(_))
    ));
    assert!(matches!(
        bus.call("boom", "b"),
        Err(PluginError::Panicked(_))
    ));
    assert!(bus.is_disabled("boom"));
    assert!(matches!(
        bus.call("boom", "c"),
        Err(PluginError::Disabled(_))
    ));
    assert_eq!(bus.call("echo", "y").unwrap(), "echo:y");
    assert!(matches!(
        bus.call("missing", ""),
        Err(PluginError::NotFound(_))
    ));
}

#[test]
fn plugin_failures_count_and_success_resets_them() {
    let bus = PluginBus::default();
    bus.register("flaky", |_| Err("backend down".to_owned()));
    for _ in 0..3 {
        assert!(matches!(
            bus.call("flaky", ""),
            Err(PluginError::Failed(_))
        ));
    }
    assert!(matches!(
        bus.call("flaky", ""),
        Err(PluginError::Disabled(_))
    ));

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let resetting = PluginBus::new(2);
    resetting.register("alternating", move |_| {
        if counter.fetch_add(1, Ordering::SeqCst) % 2 == 0 {
            Err("fail".to_owned())
        } else {
            Ok("ok".to_owned())
        }
    });
    assert!(resetting.call("alternating", "").is_err());
    assert_eq!(resetting.call("alternating", "").unwrap(), "ok");
    assert!(resetting.call("alternating", "").is_err());
    assert!(!resetting.is_disabled("alternating"));
}

#[test]
fn high_priority_task_wins() {
    let scheduler = Scheduler::new(1);
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let (started_tx, started_rx) = mpsc::channel::<()>();

    let gate_order = Arc::clone(&order);
    let gate = scheduler
        .spawn(0, move || {
            started_tx.send(()).unwrap();
            gate_rx.recv().unwrap();
            gate_order.lock().unwrap().push("gate");
        })
        .unwrap();
    started_rx.recv().unwrap();
    let low_order = Arc::clone(&order);
    let low = scheduler
        .spawn(1, move || {
            low_order.lock().unwrap().push("low");
            "low"
        })
        .unwrap();
    let high_order = Arc::clone(&order);
    let high = scheduler
        .spawn(10, move || {
            high_order.lock().unwrap().push("high");
            "high"
        })
        .unwrap();

    gate_tx.send(()).unwrap();
    gate.join().unwrap();
    assert_eq!(low.join().unwrap(), "low");
    assert_eq!(high.join().unwrap(), "high");
    assert_eq!(*order.lock().unwrap(), vec!["gate", "high", "low"]);
    scheduler.shutdown();
}

#[test]
fn all_tasks_complete_and_panicking_task_does_not_kill_worker() {
    let scheduler = Scheduler::new(4);
    let handles: Vec<_> = (0..32i32)
        .map(|i| scheduler.spawn((i % 5) as u8, move || i * 2).unwrap())
        .collect();
    let results: Vec<i32> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results, (0..32i32).map(|i| i * 2).collect::<Vec<_>>());

    let boom = scheduler
        .spawn(1, || -> i32 { panic!("task exploded") })
        .unwrap();
    assert!(matches!(boom.join(), Err(SchedulerError::TaskFailed)));
    let survivor = scheduler.spawn(1, || 42).unwrap();
    assert_eq!(survivor.join().unwrap(), 42);
    scheduler.shutdown();
}

#[test]
fn shutdown_drains_queued_work_and_rejects_new_work() {
    let scheduler = Scheduler::new(1);
    let handles: Vec<_> = (0..8i32)
        .map(|i| scheduler.spawn(1, move || i).unwrap())
        .collect();
    scheduler.shutdown();
    for (i, handle) in handles.into_iter().enumerate() {
        assert_eq!(handle.join().unwrap(), i as i32);
    }
    assert!(matches!(
        scheduler.spawn(1, || 0),
        Err(SchedulerError::ShuttingDown)
    ));
    scheduler.shutdown();
}

#[test]
fn host_degrades_after_three_consecutive_errors() {
    let mut host = Host::new();
    assert!(!host.degraded());
    assert_eq!(host.active_adapter(), "primary");

    let failing = || Err::<i32, AdapterFault>(AdapterFault("present failed".to_owned()));
    for expected in 1..=3 {
        let error = host.run(failing, || Ok(0)).unwrap_err();
        assert_eq!(error.0, "present failed");
        assert_eq!(host.consecutive_errors(), expected);
    }
    assert!(host.degraded());
    assert_eq!(host.active_adapter(), "headless");

    let primary_calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&primary_calls);
    let primary = move || -> Result<i32, AdapterFault> {
        counter.fetch_add(1, Ordering::SeqCst);
        panic!("degraded host must not call the primary adapter")
    };
    assert_eq!(host.run(primary, || Ok(9)).unwrap(), 9);
    assert_eq!(primary_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn host_success_resets_consecutive_errors() {
    let mut host = Host::new();
    let failing = || Err::<i32, AdapterFault>(AdapterFault("x".to_owned()));
    let working = || Ok::<i32, AdapterFault>(1);
    host.run(failing, || Ok(0)).unwrap_err();
    host.run(failing, || Ok(0)).unwrap_err();
    assert_eq!(host.consecutive_errors(), 2);
    assert_eq!(host.run(working, || Ok(0)).unwrap(), 1);
    assert_eq!(host.consecutive_errors(), 0);
    host.run(failing, || Ok(0)).unwrap_err();
    assert!(!host.degraded());
}

#[test]
fn cli_drives_ir_from_a_file_and_stdin() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/counter.opuir.json"
    );
    let (code, stdout, stderr) = run_cli(
        path,
        "tap 4\ninput 1 hello\nstate\nnonsense\nquit\n",
    );
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains(r#"state [{"name":"count","value":0}] version 0"#),
        "{stdout}"
    );
    assert!(
        stdout.contains(r#"patches [{"name":"count","value":1}] version 1"#),
        "{stdout}"
    );
    assert!(stdout.contains("has no input event binding"), "{stdout}");
    assert!(stdout.contains("unknown command"), "{stdout}");
    assert!(
        stdout.contains(r#"state [{"name":"count","value":1}] version 1"#),
        "{stdout}"
    );
}

#[test]
fn cli_input_and_toggle_commands() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/form.opuir.json"
    );
    let (code, stdout, stderr) = run_cli(path, "input 1 two words\nstate\ntap 2\nquit\n");
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains(r#"patches [{"name":"label","value":"two words"}] version 1"#),
        "{stdout}"
    );
    assert!(
        stdout.contains(r#"patches [{"name":"flag","value":true}] version 2"#),
        "{stdout}"
    );
}

#[test]
fn cli_rejects_bad_ir_and_missing_arg() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/bad-version.opuir.json"
    );
    let (code, _, stderr) = run_cli(path, "quit\n");
    assert_eq!(code, 1);
    assert!(stderr.contains("unsupported IR version"), "{stderr}");

    let output = Command::new(env!("CARGO_BIN_EXE_opurom-core"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
