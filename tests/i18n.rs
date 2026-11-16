//! i18n v1 `@locale` patch surface (`IR_ADDITIONS.md` §15.4): a reserved
//! patch switches the declared active locale; like `@nav`/`@validate`
//! it never enters the state mirror, and an unsupported tag is a typed error
//! that leaves the REPL alive. Programs without `locale` keep their bytes.

use opurom_core::{CoreError, Event, Patch, Runtime, Value};
use serde_json::{json, Value as Json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn ir_with_locale() -> String {
    json!({
        "ir": "0.1.0", "app": "I18n",
        "state": [{"name": "count", "type": "int", "init": 0}],
        "locale": {"active": "en", "supported": ["en", "fr", "zh-Hans-CN"]},
        "messages": {"greeting": {"en": "Hello", "fr": "Bonjour"}},
        "pages": ["Home"],
        "tree": {"id": 0, "kind": "Column", "props": {}, "events": {}, "children": [
            {"id": 1, "kind": "Button", "props": {"label": "go"}, "children": [],
             "events": {"tap": {"op": "add", "target": "count", "value": 1}}}
        ]}
    })
    .to_string()
}

fn ir_without_locale() -> String {
    let mut ir: Json = serde_json::from_str(&ir_with_locale()).unwrap();
    ir.as_object_mut().unwrap().remove("locale");
    ir.as_object_mut().unwrap().remove("messages");
    ir.to_string()
}

fn locale_patch(tag: &str) -> Patch {
    Patch {
        name: "@locale".to_owned(),
        value: Value::Str(tag.to_owned()),
    }
}

#[test]
fn a_locale_patch_switches_the_active_locale_and_bumps_the_version() {
    let mut runtime = Runtime::from_ir_json(&ir_with_locale()).unwrap();
    assert_eq!(runtime.locale().unwrap().active, "en");

    let applied = runtime.apply_patch(&locale_patch("fr")).unwrap();
    assert_eq!(applied, locale_patch("fr"));
    assert_eq!(runtime.locale().unwrap().active, "fr");
    assert_eq!(runtime.version(), 1);

    // A second switch keeps working and keeps bumping the version.
    runtime.apply_patch(&locale_patch("zh-Hans-CN")).unwrap();
    assert_eq!(runtime.locale().unwrap().active, "zh-Hans-CN");
    assert_eq!(runtime.version(), 2);

    // Reserved patches are not state: no mirror entry, no state-name lookup.
    assert_eq!(runtime.state("@locale"), None);
    assert!(
        runtime
            .snapshot()
            .iter()
            .all(|patch| patch.name != "@locale"),
        "state mirror must exclude the reserved patch"
    );
    assert_eq!(runtime.snapshot().len(), 1);
}

#[test]
fn an_unsupported_tag_is_a_typed_error_without_mutation() {
    let mut runtime = Runtime::from_ir_json(&ir_with_locale()).unwrap();
    let error = runtime.apply_patch(&locale_patch("de")).unwrap_err();
    assert!(
        matches!(&error, CoreError::InvalidLocale { message } if message.contains("not supported") && message.contains("de")),
        "{error:?}"
    );
    assert!(error.to_string().starts_with("@locale "), "{error}");
    assert_eq!(runtime.locale().unwrap().active, "en");
    assert_eq!(runtime.version(), 0);

    // The runtime is still usable: a valid patch and a normal event both work.
    runtime.apply_patch(&locale_patch("fr")).unwrap();
    runtime.dispatch(Event::Tap { node_id: 1 }).unwrap();
    assert_eq!(runtime.locale().unwrap().active, "fr");
    assert_eq!(runtime.state("count"), Some(&Value::Int(1)));
}

#[test]
fn a_non_string_patch_value_is_a_typed_error() {
    let mut runtime = Runtime::from_ir_json(&ir_with_locale()).unwrap();
    let error = runtime
        .apply_patch(&Patch {
            name: "@locale".to_owned(),
            value: Value::Int(1),
        })
        .unwrap_err();
    assert!(
        matches!(&error, CoreError::InvalidLocale { message } if message.contains("value must be a string")),
        "{error:?}"
    );
    assert_eq!(runtime.version(), 0);
}

#[test]
fn a_program_without_a_locale_cannot_switch_one() {
    let mut runtime = Runtime::from_ir_json(&ir_without_locale()).unwrap();
    assert!(runtime.locale().is_none());
    let error = runtime.apply_patch(&locale_patch("fr")).unwrap_err();
    assert!(
        matches!(&error, CoreError::InvalidLocale { message } if message.contains("requires a declared locale")),
        "{error:?}"
    );
    assert_eq!(runtime.version(), 0);
    assert_eq!(runtime.snapshot().len(), 1);
}

#[test]
fn the_declared_locale_shape_is_validated_at_parse_time() {
    for (active, supported) in [
        ("de", json!(["en", "fr"])),          // active not supported
        ("en", json!([])),                    // empty supported
        ("EN", json!(["EN", "fr"])),          // primary must be lowercase
        ("english", json!(["english"])),      // primary too long
        ("en", json!(["en", "x-123456789"])), // subtag too long
    ] {
        let ir = json!({
            "ir": "0.1.0", "app": "Bad", "state": [],
            "locale": {"active": active, "supported": supported},
            "pages": [], "tree": {"id": 0, "kind": "Column", "props": {}, "events": {}, "children": []}
        })
        .to_string();
        let error = Runtime::from_ir_json(&ir).unwrap_err();
        assert!(
            matches!(error, CoreError::InvalidIr(_)),
            "bad locale {active}: {error:?}"
        );
    }
}

fn scratch_ir(ir: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "opurom-locale-{}-{}.opuir.json",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, ir).unwrap();
    path
}

fn run_cli(path: &Path, input: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_opurom-core"))
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("cli starts");
    child
        .stdin
        .as_mut()
        .expect("stdin pipe")
        .write_all(input.as_bytes())
        .expect("stdin write");
    let output = child.wait_with_output().expect("cli runs");
    String::from_utf8(output.stdout).expect("stdout utf-8")
}

#[test]
fn the_cli_switches_locales_and_answers_bad_patches_on_the_error_line() {
    let path = scratch_ir(&ir_with_locale());
    let out = run_cli(
        &path,
        "patch {\"name\":\"@locale\",\"value\":\"fr\"}\n\
         patch {\"name\":\"@locale\",\"value\":\"de\"}\n\
         patch not-json\n\
         patch {\"name\":\"@locale\",\"value\":1}\nstate\nquit\n",
    );
    let lines: Vec<&str> = out.lines().collect();

    assert!(
        lines
            .iter()
            .any(|line| *line == "patch {\"name\":\"@locale\",\"value\":\"fr\"} version 1"),
        "successful patch ack: {out}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("error: @locale") && line.contains("not supported")),
        "typed error line: {out}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("error: patch payload is not valid JSON")),
        "bad json: {out}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("error: @locale value must be a string")),
        "bad value: {out}"
    );
    // Kernel stayed alive, and @locale is absent from the state mirror.
    let state = lines
        .iter()
        .find(|line| line.starts_with("state "))
        .expect("state line");
    assert!(!state.contains("@locale"), "{state}");
    assert!(
        state.contains("\"version\"") || state.contains("\"name\":\"count\""),
        "{state}"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn without_a_locale_the_cli_output_bytes_are_unchanged() {
    let path = scratch_ir(&ir_without_locale());
    let plain = run_cli(&path, "layers\nstate\nquit\n");
    let with_bad_patch = run_cli(
        &path,
        "patch {\"name\":\"@locale\",\"value\":\"fr\"}\nlayers\nstate\nquit\n",
    );
    let line_of = |out: &str, prefix: &str| -> String {
        out.lines()
            .find(|line| line.starts_with(prefix))
            .expect("expected output line")
            .to_owned()
    };
    assert_eq!(
        line_of(&plain, "layers "),
        line_of(&with_bad_patch, "layers ")
    );
    assert_eq!(
        line_of(&plain, "state "),
        line_of(&with_bad_patch, "state ")
    );
    assert!(
        with_bad_patch.contains("error: @locale \"fr\" requires a declared locale"),
        "{with_bad_patch}"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn a_locale_switch_never_changes_the_kernel_render_bytes() {
    let path = scratch_ir(&ir_with_locale());
    let plain = run_cli(&path, "layers\nquit\n");
    let switched = run_cli(
        &path,
        "patch {\"name\":\"@locale\",\"value\":\"fr\"}\nlayers\nquit\n",
    );
    let layers_of = |out: &str| -> String {
        let line = out
            .lines()
            .find(|line| line.starts_with("layers "))
            .expect("layers line");
        // Only the version suffix changes: the render payload stays identical.
        line["layers ".len()..]
            .rsplit_once(" version")
            .unwrap()
            .0
            .to_owned()
    };
    assert_eq!(layers_of(&plain), layers_of(&switched));
    let _ = fs::remove_file(&path);
}

/// The compiler emits `set locale: "fr"` as an event action targeting the
/// reserved `@locale` patch; the loader must admit it without treating it as
/// a state name (`IR_ADDITIONS.md` §15.4).
fn ir_with_locale_event(action: Json) -> String {
    json!({
        "ir": "0.1.0", "app": "I18nEvent",
        "state": [{"name": "count", "type": "int", "init": 0}],
        "locale": {"active": "en", "supported": ["en", "fr", "fr-CH"]},
        "pages": ["Home"],
        "tree": {"id": 0, "kind": "Column", "props": {}, "events": {}, "children": [
            {"id": 1, "kind": "Button", "props": {"label": "switch"}, "children": [],
             "events": {"tap": action}}
        ]}
    })
    .to_string()
}

#[test]
fn a_compiled_set_locale_event_loads_without_entering_the_state_mirror() {
    let ir = ir_with_locale_event(json!({"op": "set", "target": "@locale", "value": "fr"}));
    let runtime = Runtime::from_ir_json(&ir).unwrap();
    assert_eq!(runtime.locale().unwrap().active, "en");
    assert_eq!(runtime.state("@locale"), None);
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].name, "count");
}

#[test]
fn a_set_locale_event_inside_an_action_list_loads_too() {
    let ir = ir_with_locale_event(json!({"actions": [
        {"op": "set", "target": "@locale", "value": "fr-CH"},
        {"op": "add", "target": "count", "value": 1}
    ]}));
    let runtime = Runtime::from_ir_json(&ir).unwrap();
    assert_eq!(runtime.locale().unwrap().active, "en");
    assert_eq!(runtime.snapshot().len(), 1);
}

#[test]
fn a_set_locale_event_with_a_bad_tag_is_rejected_at_load() {
    let ir = ir_with_locale_event(json!({"op": "set", "target": "@locale", "value": "not a tag!"}));
    let error = Runtime::from_ir_json(&ir).unwrap_err();
    assert!(
        matches!(&error, CoreError::InvalidLocale { message } if message.contains("BCP-47")),
        "{error:?}"
    );
    assert!(error.to_string().starts_with("@locale "), "{error}");

    let from =
        ir_with_locale_event(json!({"op": "set", "target": "@locale", "valueFrom": "count"}));
    let error = Runtime::from_ir_json(&from).unwrap_err();
    assert!(
        matches!(&error, CoreError::InvalidIr(message) if message.contains("literal tag")),
        "{error:?}"
    );
}

#[test]
fn other_ops_and_state_targets_still_require_declared_state() {
    let other_op =
        ir_with_locale_event(json!({"op": "append", "target": "@locale", "value": "fr"}));
    let error = Runtime::from_ir_json(&other_op).unwrap_err();
    assert!(
        matches!(&error, CoreError::InvalidIr(message) if message.contains("not declared in state")),
        "{error:?}"
    );

    let ghost = ir_with_locale_event(json!({"op": "set", "target": "ghost", "value": "fr"}));
    let error = Runtime::from_ir_json(&ghost).unwrap_err();
    assert!(
        matches!(&error, CoreError::InvalidIr(message) if message.contains("not declared in state")),
        "{error:?}"
    );
}

#[test]
fn a_dispatched_set_locale_event_emits_the_reserved_patch_and_switches() {
    // Same event shape the lang compiler emits for `set locale: "fr"`.
    let ir = ir_with_locale_event(json!({"op": "set", "target": "@locale", "value": "fr"}));
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 1 }).unwrap();

    assert_eq!(
        patches,
        vec![Patch {
            name: "@locale".to_owned(),
            value: Value::Str("fr".to_owned()),
        }]
    );
    assert_eq!(runtime.version(), 1);
    assert_eq!(runtime.locale().unwrap().active, "fr");
    // The reserved patch goes to the consumer, never into the state mirror.
    assert_eq!(runtime.state("@locale"), None);
    assert_eq!(runtime.snapshot().len(), 1);
    assert_eq!(runtime.snapshot()[0].name, "count");
}

#[test]
fn a_dispatched_locale_switch_shares_one_action_list_with_state_ops() {
    let ir = ir_with_locale_event(json!({"actions": [
        {"op": "set", "target": "@locale", "value": "fr-CH"},
        {"op": "add", "target": "count", "value": 1}
    ]}));
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 1 }).unwrap();
    assert_eq!(patches.len(), 2);
    assert_eq!(patches[0].name, "@locale");
    assert_eq!(patches[1].name, "count");
    assert_eq!(patches[1].value, Value::Int(1));
    assert_eq!(runtime.locale().unwrap().active, "fr-CH");
    assert_eq!(runtime.state("count"), Some(&Value::Int(1)));
    assert_eq!(runtime.version(), 1);
    assert_eq!(runtime.snapshot().len(), 1);
}

#[test]
fn an_unsupported_tag_at_dispatch_is_a_typed_error_without_mutation() {
    // `de` is a valid BCP-47 tag but not in this program's supported list.
    let ir = ir_with_locale_event(json!({"op": "set", "target": "@locale", "value": "de"}));
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let error = runtime.dispatch(Event::Tap { node_id: 1 }).unwrap_err();
    assert!(
        matches!(&error, CoreError::InvalidLocale { message } if message.contains("not supported") && message.contains("de")),
        "{error:?}"
    );
    assert_eq!(runtime.locale().unwrap().active, "en");
    assert_eq!(runtime.version(), 0);
    assert_eq!(runtime.state("count"), Some(&Value::Int(0)));
}

#[test]
fn a_locale_event_in_a_program_without_a_locale_errors_at_dispatch() {
    let ir = json!({
        "ir": "0.1.0", "app": "NoLocale", "state": [],
        "pages": ["Home"],
        "tree": {"id": 0, "kind": "Column", "props": {}, "events": {}, "children": [
            {"id": 1, "kind": "Button", "props": {}, "children": [],
             "events": {"tap": {"op": "set", "target": "@locale", "value": "fr"}}}
        ]}
    })
    .to_string();
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let error = runtime.dispatch(Event::Tap { node_id: 1 }).unwrap_err();
    assert!(
        matches!(&error, CoreError::InvalidLocale { message } if message.contains("requires a declared locale")),
        "{error:?}"
    );
    assert_eq!(runtime.version(), 0);
}
