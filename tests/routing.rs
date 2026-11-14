use opurom_core::{CoreError, Event, Ir, Patch, Runtime, Value};
use serde_json::{json, Value as Json};

fn routes_ir(routes: Json, initial: Json, events: Json) -> String {
    json!({
        "ir": "0.1.0",
        "app": "Router",
        "state": [],
        "initialRoute": initial,
        "routes": routes,
        "theme": {}
    })
    .to_string()
    .replace("__EVENTS__", &events.to_string())
}

/// A minimal node with the given id carrying the given tap handler.
fn node(id: u32, kind: &str, events: Json) -> Json {
    json!({
        "id": id,
        "kind": kind,
        "props": {},
        "children": [],
        "events": events
    })
}

// (a) A multi-page program parses its routes and initialRoute, with per-page
// trees, and no top-level tree.
#[test]
fn parses_routes_and_initial_route() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "Shop",
        "state": [],
        "initialRoute": "Home",
        "routes": {
            "Home": {"tree": node(0, "Column", json!({})), "title": "Home"},
            "Detail": {"tree": node(1, "Column", json!({})), "title": "Detail"}
        },
        "theme": {}
    })
    .to_string();
    let parsed = Ir::parse(&ir).unwrap();
    assert_eq!(parsed.initial_route.as_deref(), Some("Home"));
    let mut names: Vec<&str> = parsed.routes.iter().map(|r| r.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["Detail", "Home"]);
    let home = parsed.routes.iter().find(|r| r.name == "Home").unwrap();
    let detail = parsed.routes.iter().find(|r| r.name == "Detail").unwrap();
    assert_eq!(home.title.as_deref(), Some("Home"));
    // Each route keeps its own tree.
    assert_eq!(home.tree.id, 0);
    assert_eq!(detail.tree.id, 1);
    assert_eq!(parsed.pages.len(), 2);
}

// Route titles are optional.
#[test]
fn route_title_is_optional() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "initialRoute": "Home",
        "routes": {"Home": {"tree": node(0, "Column", json!({}))}},
        "theme": {}
    })
    .to_string();
    let parsed = Ir::parse(&ir).unwrap();
    assert_eq!(parsed.routes[0].title, None);
}

#[test]
fn initial_route_must_reference_a_declared_route() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "initialRoute": "Ghost",
        "routes": {"Home": {"tree": node(0, "Column", json!({}))}},
        "theme": {}
    })
    .to_string();
    let err = Ir::parse(&ir).unwrap_err();
    assert!(err.to_string().contains("not a declared route"), "{err}");
}

#[test]
fn routes_without_initial_route_is_rejected() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "routes": {"Home": {"tree": node(0, "Column", json!({}))}},
        "theme": {}
    })
    .to_string();
    let err = Ir::parse(&ir).unwrap_err();
    assert!(err.to_string().contains("initialRoute"), "{err}");
}

#[test]
fn a_route_must_carry_a_tree_object() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "initialRoute": "Home",
        "routes": {"Home": {"title": "Home"}},
        "theme": {}
    })
    .to_string();
    let err = Ir::parse(&ir).unwrap_err();
    assert!(err.to_string().contains("tree"), "{err}");
}

#[test]
fn routes_with_a_top_level_tree_is_rejected() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "initialRoute": "Home",
        "tree": node(0, "Column", json!({})),
        "routes": {"Home": {"tree": node(1, "Column", json!({}))}},
        "theme": {}
    })
    .to_string();
    let err = Ir::parse(&ir).unwrap_err();
    assert!(err.to_string().contains("illegal"), "{err}");
}

#[test]
fn node_ids_are_unique_across_routes() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "initialRoute": "Home",
        "routes": {
            "Home": {"tree": node(7, "Column", json!({}))},
            "Detail": {"tree": node(7, "Column", json!({}))}
        },
        "theme": {}
    })
    .to_string();
    let err = Ir::parse(&ir).unwrap_err();
    assert!(err.to_string().contains("duplicate node id"), "{err}");
}

// (b) The pop op parses and dispatches a reserved @nav signal.
#[test]
fn pop_parses_and_dispatches_a_signal() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "initialRoute": "Home",
        "routes": {
            "Home": {"tree": node(0, "Button", json!({"tap": {"op": "pop"}}))},
            "Detail": {"tree": node(1, "Column", json!({}))}
        },
        "theme": {}
    })
    .to_string();
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(
        patches,
        vec![Patch {
            name: "@nav".to_string(),
            value: Value::Str("..".to_string()),
        }]
    );
    assert_eq!(runtime.version(), 1);
    assert!(runtime.snapshot().is_empty(), "pop leaves state intact");
}

#[test]
fn pop_with_a_target_is_rejected() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "initialRoute": "Home",
        "routes": {"Home": {"tree": node(0, "Button", json!({"tap": {"op": "pop", "target": "Home"}}))}},
        "theme": {}
    })
    .to_string();
    let err = Ir::parse(&ir).unwrap_err();
    assert!(err.to_string().contains("pop must not"), "{err}");
}

// (c) When routes are present, a push target must be a declared route.
#[test]
fn push_to_an_undeclared_route_is_rejected() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "initialRoute": "Home",
        "routes": {
            "Home": {"tree": node(0, "Button", json!({"tap": {"op": "push", "target": "Ghost"}}))},
            "Detail": {"tree": node(1, "Column", json!({}))}
        },
        "theme": {}
    })
    .to_string();
    let err = Ir::parse(&ir).unwrap_err();
    assert!(matches!(err, CoreError::InvalidIr(_)), "{err:?}");
    assert!(err.to_string().contains("not a declared route"), "{err}");
}

#[test]
fn push_to_a_declared_route_dispatches_a_signal() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "initialRoute": "Home",
        "routes": {
            "Home": {"tree": node(0, "Button", json!({"tap": {"op": "push", "target": "Detail"}}))},
            "Detail": {"tree": node(1, "Column", json!({}))}
        },
        "theme": {}
    })
    .to_string();
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(patches[0].name, "@nav");
    assert_eq!(patches[0].value, Value::Str("Detail".to_string()));
}

#[test]
fn runtime_exposes_routes_with_titles_and_initial_route() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "initialRoute": "Home",
        "routes": {
            "Home": {"tree": node(0, "Column", json!({})), "title": "Home"},
            "Detail": {"tree": node(1, "Column", json!({})), "title": "Detail"}
        },
        "theme": {}
    })
    .to_string();
    let runtime = Runtime::from_ir_json(&ir).unwrap();
    assert_eq!(runtime.initial_route(), Some("Home"));
    let records = runtime.page_records();
    // Route order follows the IR object keys (alphabetical without
    // serde_json preserve_order), so compare as an unordered set.
    assert_eq!(
        records,
        Json::Array(vec![
            json!({"name": "Detail", "title": "Detail"}),
            json!({"name": "Home", "title": "Home"}),
        ])
    );
}

// (d) A single-page 0.1.0 program parses unchanged (guard).
#[test]
fn single_page_ir_is_unchanged() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "Counter",
        "state": [{"name": "count", "type": "int", "init": 0}],
        "tree": node(0, "Button", json!({"tap": {"op": "add", "target": "count", "value": 1}})),
        "theme": {}
    })
    .to_string();
    let parsed = Ir::parse(&ir).unwrap();
    assert_eq!(parsed.version, "0.1.0");
    assert!(parsed.routes.is_empty());
    assert_eq!(parsed.initial_route, None);
    let mut runtime = Runtime::from_ir_json(&ir).unwrap();
    assert_eq!(runtime.initial_route(), None);
    let patches = runtime.dispatch(Event::Tap { node_id: 0 }).unwrap();
    assert_eq!(patches[0].name, "count");
    assert_eq!(patches[0].value, Value::Int(1));
}

// Each route's content tree is reachable through the runtime, so a renderer
// can draw the page it navigated to.
#[test]
fn route_tree_exposes_per_page_content() {
    let ir = json!({
        "ir": "0.1.0",
        "app": "T",
        "state": [],
        "initialRoute": "Home",
        "routes": {
            "Home": {"tree": node(0, "Column", json!({}))},
            "Detail": {"tree": node(1, "Button", json!({}))}
        },
        "theme": {}
    })
    .to_string();
    let runtime = Runtime::from_ir_json(&ir).unwrap();
    assert_eq!(runtime.route_tree("Home").unwrap().id, 0);
    assert_eq!(runtime.route_tree("Detail").unwrap().kind, "Button");
    assert!(runtime.route_tree("Ghost").is_none());
}

// The `routes_ir` helper builds a well-formed multi-page document; a sanity check
// keeps it honest.
#[test]
fn routes_ir_helper_builds_parseable_ir() {
    let ir = routes_ir(
        json!({"Home": {"tree": node(0, "Column", json!({}))}}),
        json!("Home"),
        json!({}),
    );
    assert!(Ir::parse(&ir).is_ok());
}
