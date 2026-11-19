use crate::error::CoreError;
use crate::state::Value;
use serde_json::{Map, Value as Json};
use std::collections::{HashMap, HashSet};

pub const IR_VERSION: &str = "0.1.0";

/// Author-declarable IR event names. The set is closed: `dragEnd` and `nav`
/// are runtime gesture-binding aliases, not node events.
pub const EVENT_NAMES: &[&str] = &["tap", "input", "doubleTap", "longPress", "drag"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Backspace,
    Toggle,
    Append,
    RemoveAt,
    Assign,
    Push,
    Pop,
    Validate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub op: Op,
    pub target: String,
    pub value: Option<Value>,
    /// State whose current value is read when this action executes.
    pub value_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: u32,
    pub kind: String,
    pub props: Map<String, Json>,
    pub children: Vec<Node>,
    pub when: Option<String>,
    pub events: HashMap<String, Vec<Action>>,
}

impl Node {
    /// A placeholder tree for a multi-page program, whose content lives in `routes`
    /// rather than a single top-level `tree`.
    fn empty() -> Node {
        Node {
            id: 0,
            kind: String::new(),
            props: Map::new(),
            children: Vec::new(),
            when: None,
            events: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDecl {
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locale {
    pub active: String,
    pub supported: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub name: String,
    /// The page's content tree. Each route carries its own tree so the IR can
    /// describe a real multi-page app, not a single hero screen.
    pub tree: Node,
    /// Optional page title, exposed to the renderer.
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ir {
    pub version: String,
    pub app: String,
    pub state: Vec<StateDecl>,
    /// Declared page names in declaration order. For a multi-page program this
    /// is the routes; for a single-page program it is the legacy flat
    /// `pages` list.
    pub pages: Vec<String>,
    /// Only present for a single-page program; empty for a multi-page
    /// program, whose content lives in `routes`.
    pub tree: Node,
    /// Routes in declaration order (empty for a single-page program).
    pub routes: Vec<Route>,
    /// Initial route name (None for a single-page program).
    pub initial_route: Option<String>,
    pub theme: Json,
    /// Optional root locale object (IR_ADDITIONS §15.1); absent in older IR.
    pub locale: Option<Locale>,
}

fn invalid(message: impl std::fmt::Display) -> CoreError {
    CoreError::InvalidIr(message.to_string())
}

fn field<'a>(object: &'a Map<String, Json>, key: &str, at: &str) -> Result<&'a Json, CoreError> {
    object
        .get(key)
        .ok_or_else(|| invalid(format!("{at} is missing {key:?}")))
}

fn string_field(object: &Map<String, Json>, key: &str, at: &str) -> Result<String, CoreError> {
    field(object, key, at)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("{at}.{key} must be a string")))
}

fn typed_value(json: &Json, declared: &Value, name: &str) -> Result<Value, CoreError> {
    let parsed = match declared {
        Value::Int(_) => json.as_i64().map(Value::Int),
        Value::Bool(_) => json.as_bool().map(Value::Bool),
        Value::Str(_) => json.as_str().map(|s| Value::Str(s.to_owned())),
        // Unreachable: a list target is rejected before `set` parses a value.
        Value::List(_) => None,
    };
    parsed.ok_or_else(|| CoreError::TypeMismatch {
        name: name.to_owned(),
        expected: declared.type_name(),
    })
}

fn same_declared_type(left: &Value, right: &Value) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

/// Parses a Phase A list value: an array of strings, element by element.
fn string_array(json: &Json, at: &str) -> Result<Vec<String>, CoreError> {
    let array = json
        .as_array()
        .ok_or_else(|| invalid(format!("{at} must be an array")))?;
    array
        .iter()
        .enumerate()
        .map(|(i, item)| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid(format!("{at}[{i}] must be a string")))
        })
        .collect()
}

/// The `valueFrom` of an `append`: a declared string state, since list
/// elements are strings.
fn validate_string_from(
    object: &Map<String, Json>,
    types: &HashMap<String, Value>,
    at: &str,
) -> Result<String, CoreError> {
    let source = object
        .get("valueFrom")
        .and_then(Json::as_str)
        .ok_or_else(|| invalid(format!("{at}.valueFrom must be a string state name")))?
        .to_owned();
    match types.get(&source) {
        Some(Value::Str(_)) => Ok(source),
        Some(declared) => Err(invalid(format!(
            "{at}: append valueFrom {source:?} must be a string state, it is {}",
            declared.type_name()
        ))),
        None => Err(invalid(format!(
            "{at}.valueFrom {source:?} is not declared in state"
        ))),
    }
}

fn parse_state(items: &[Json]) -> Result<(Vec<StateDecl>, HashMap<String, Value>), CoreError> {
    let mut decls = Vec::with_capacity(items.len());
    let mut types = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        let at = format!("state[{i}]");
        let object = item
            .as_object()
            .ok_or_else(|| invalid(format!("{at} must be an object")))?;
        let name = string_field(object, "name", &at)?;
        let kind = string_field(object, "type", &at)?;
        if name.is_empty() {
            return Err(invalid(format!("{at}.name must not be empty")));
        }
        let init = field(object, "init", &at)?;
        let value = match kind.as_str() {
            "int" => Value::Int(
                init.as_i64()
                    .ok_or_else(|| invalid(format!("{at}.init must be an integer for type int")))?,
            ),
            "bool" => Value::Bool(
                init.as_bool()
                    .ok_or_else(|| invalid(format!("{at}.init must be a bool for type bool")))?,
            ),
            "string" => Value::Str(
                init.as_str()
                    .ok_or_else(|| invalid(format!("{at}.init must be a string for type string")))?
                    .to_owned(),
            ),
            // A Phase A list is an array of strings; an empty init means a list
            // that fills in at runtime, so no element is required up front.
            "list" => Value::List(string_array(init, &format!("{at}.init"))?),
            _ => {
                return Err(invalid(format!(
                    "{at}.type {kind:?} is not int, bool, string or list"
                )))
            }
        };
        if types.insert(name.clone(), value.clone()).is_some() {
            return Err(invalid(format!("duplicate state name {name:?}")));
        }
        decls.push(StateDecl { name, value });
    }
    Ok((decls, types))
}

fn validate_value_from(
    object: &Map<String, Json>,
    target: &str,
    declared: &Value,
    types: &HashMap<String, Value>,
    at: &str,
    require_int: bool,
) -> Result<String, CoreError> {
    let source = object
        .get("valueFrom")
        .and_then(Json::as_str)
        .ok_or_else(|| invalid(format!("{at}.valueFrom must be a string state name")))?
        .to_owned();
    let source_declared = types.get(&source).ok_or_else(|| {
        invalid(format!(
            "{at}.valueFrom {source:?} is not declared in state"
        ))
    })?;
    if require_int && !matches!(source_declared, Value::Int(_)) {
        return Err(invalid(format!(
            "{at}: requires an int valueFrom source, {source:?} is {}",
            source_declared.type_name()
        )));
    }
    if !same_declared_type(declared, source_declared) {
        return Err(invalid(format!(
            "{at}: valueFrom {source:?} must have the same declared type as {target:?}"
        )));
    }
    Ok(source)
}

fn parse_action(
    json: &Json,
    event: &str,
    types: &HashMap<String, Value>,
    routes: Option<(&[String], &str)>,
    at: &str,
) -> Result<Action, CoreError> {
    let object = json
        .as_object()
        .ok_or_else(|| invalid(format!("{at} must be an object")))?;
    let op = string_field(object, "op", at)?;
    // pop unwinds the navigation stack to the previous page: it names no page
    // and carries no value.
    if op == "pop" {
        if object.contains_key("target") || object.contains_key("value") {
            return Err(invalid(format!(
                "{at}: pop must not have a target or value"
            )));
        }
        if object.contains_key("valueFrom") {
            return Err(invalid(format!("{at}: pop must not have a value")));
        }
        return Ok(Action {
            op: Op::Pop,
            target: String::new(),
            value: None,
            value_from: None,
        });
    }
    let target = string_field(object, "target", at)?;
    // Navigation is a signal, not a state mutation: its target is a declared
    // page, and it carries no value.
    if op == "push" {
        if let Some((names, noun)) = routes {
            if !names.iter().any(|name| name == &target) {
                return Err(invalid(format!(
                    "{at}.target {target:?} is not a declared {noun}"
                )));
            }
        }
        if object.contains_key("value") || object.contains_key("valueFrom") {
            return Err(invalid(format!("{at}: push must not have a value")));
        }
        return Ok(Action {
            op: Op::Push,
            target,
            value: None,
            value_from: None,
        });
    }
    // Form validation is a built-in signal: the target names the form scope
    // and the action carries no value, mirroring the compiled targets.
    if op == "validate" {
        if object.contains_key("value") || object.contains_key("valueFrom") {
            return Err(invalid(format!("{at}: validate must not have a value")));
        }
        return Ok(Action {
            op: Op::Validate,
            target,
            value: None,
            value_from: None,
        });
    }
    // `set @locale` is the compiler's event spelling of the reserved locale
    // patch (`IR_ADDITIONS.md` §15.4). @locale is not a state name, so it must
    // bypass the declared-state lookup; every other op keeps that lookup.
    if op == "set" && target == "@locale" {
        if object.contains_key("valueFrom") {
            return Err(invalid(format!(
                "{at}: @locale takes a literal tag, not valueFrom"
            )));
        }
        let tag = object
            .get("value")
            .and_then(Json::as_str)
            .ok_or_else(|| invalid(format!("{at}: @locale value must be a string")))?;
        if !valid_locale_tag(tag) {
            return Err(CoreError::InvalidLocale {
                message: format!("{tag:?} is not a BCP-47 tag"),
            });
        }
        return Ok(Action {
            op: Op::Set,
            target,
            value: Some(Value::Str(tag.to_owned())),
            value_from: None,
        });
    }
    let declared = types
        .get(&target)
        .ok_or_else(|| invalid(format!("{at}.target {target:?} is not declared in state")))?;
    let raw_value = object.get("value");
    let raw_from = object.get("valueFrom");
    if raw_value.is_some() && raw_from.is_some() {
        return Err(invalid(format!(
            "{at}: value and valueFrom are mutually exclusive"
        )));
    }
    let value;
    let value_from;
    match op.as_str() {
        "add" | "sub" | "mul" | "div" => {
            if !matches!(declared, Value::Int(_)) {
                return Err(invalid(format!(
                    "{at}: {op} requires an int target, {target:?} is {}",
                    declared.type_name()
                )));
            }
            if raw_from.is_some() {
                value = None;
                value_from = Some(validate_value_from(
                    object, &target, declared, types, at, true,
                )?);
            } else {
                let n = raw_value
                    .and_then(Json::as_i64)
                    .ok_or_else(|| invalid(format!("{at}: {op} requires an integer value")))?;
                value = Some(Value::Int(n));
                value_from = None;
            }
        }
        "toggle" => {
            if !matches!(declared, Value::Bool(_)) {
                return Err(invalid(format!(
                    "{at}: toggle requires a bool target, {target:?} is {}",
                    declared.type_name()
                )));
            }
            if raw_value.is_some() || raw_from.is_some() {
                return Err(invalid(format!("{at}: toggle must not have a value")));
            }
            value = None;
            value_from = None;
        }
        "append" => {
            if !matches!(declared, Value::List(_)) {
                return Err(invalid(format!(
                    "{at}: append requires a list target, {target:?} is {}",
                    declared.type_name()
                )));
            }
            match (raw_value, raw_from) {
                (Some(raw), None) => {
                    value = Some(Value::Str(
                        raw.as_str()
                            .ok_or_else(|| {
                                invalid(format!("{at}: append requires a string value"))
                            })?
                            .to_owned(),
                    ));
                    value_from = None;
                }
                (None, Some(_)) => {
                    value = None;
                    value_from = Some(validate_string_from(object, types, at)?);
                }
                (None, None) => return Err(invalid(format!("{at}: append requires a value"))),
                (Some(_), Some(_)) => {
                    return Err(invalid(format!(
                        "{at}: value and valueFrom are mutually exclusive"
                    )))
                }
            }
        }
        "removeAt" => {
            if !matches!(declared, Value::List(_)) {
                return Err(invalid(format!(
                    "{at}: removeAt requires a list target, {target:?} is {}",
                    declared.type_name()
                )));
            }
            if raw_from.is_some() {
                return Err(invalid(format!("{at}: removeAt must not have a valueFrom")));
            }
            let index = raw_value
                .and_then(Json::as_i64)
                .filter(|n| *n >= 0)
                .ok_or_else(|| {
                    invalid(format!(
                        "{at}: removeAt requires a non-negative integer value"
                    ))
                })?;
            value = Some(Value::Int(index));
            value_from = None;
        }
        "assign" => {
            if !matches!(declared, Value::List(_)) {
                return Err(invalid(format!(
                    "{at}: assign requires a list target, {target:?} is {}",
                    declared.type_name()
                )));
            }
            match (raw_value, raw_from) {
                (Some(raw), None) => {
                    value = Some(Value::List(string_array(raw, &format!("{at}.value"))?));
                    value_from = None;
                }
                (None, Some(_)) => {
                    value = None;
                    value_from = Some(validate_value_from(
                        object, &target, declared, types, at, false,
                    )?);
                }
                (None, None) => return Err(invalid(format!("{at}: assign requires a value"))),
                (Some(_), Some(_)) => {
                    return Err(invalid(format!(
                        "{at}: value and valueFrom are mutually exclusive"
                    )))
                }
            }
        }
        "backspace" => {
            if !matches!(declared, Value::Int(_) | Value::Str(_)) {
                return Err(invalid(format!(
                    "{at}: backspace requires an int or string target, {target:?} is {}",
                    declared.type_name()
                )));
            }
            if raw_value.is_some() || raw_from.is_some() {
                return Err(invalid(format!("{at}: backspace must not have a value")));
            }
            value = None;
            value_from = None;
        }
        "set" => {
            // A list is replaced whole by `assign`, so no `set` spelling
            // (literal, valueFrom or input text) may reach a list target.
            if matches!(declared, Value::List(_)) {
                return Err(invalid(format!(
                    "{at}: set cannot replace the list {target:?}; use assign"
                )));
            }
            match (raw_value, raw_from) {
                (Some(raw), None) => {
                    value = Some(typed_value(raw, declared, &target)?);
                    value_from = None;
                }
                (None, Some(_)) => {
                    value = None;
                    value_from = Some(validate_value_from(
                        object, &target, declared, types, at, false,
                    )?);
                }
                (None, None) => {
                    if event != "input" {
                        return Err(invalid(format!(
                            "{at}: set without value is only legal for input events"
                        )));
                    }
                    // The input text is parsed by the target's declared type at
                    // dispatch, so int and bool controls bind the same way as
                    // string inputs.
                    value = None;
                    value_from = None;
                }
                (Some(_), Some(_)) => {
                    return Err(invalid(format!(
                        "{at}: value and valueFrom are mutually exclusive"
                    )))
                }
            }
        }
        _ => {
            return Err(invalid(format!(
                "{at}.op {op:?} is not set, add, sub, mul, div, backspace, toggle, append, removeAt or assign"
            )))
        }
    };
    let op = match op.as_str() {
        "set" => Op::Set,
        "add" => Op::Add,
        "sub" => Op::Sub,
        "mul" => Op::Mul,
        "div" => Op::Div,
        "backspace" => Op::Backspace,
        "append" => Op::Append,
        "removeAt" => Op::RemoveAt,
        "assign" => Op::Assign,
        _ => Op::Toggle,
    };
    Ok(Action {
        op,
        target,
        value,
        value_from,
    })
}

fn parse_actions(
    json: &Json,
    event: &str,
    types: &HashMap<String, Value>,
    routes: Option<(&[String], &str)>,
    at: &str,
) -> Result<Vec<Action>, CoreError> {
    let object = json
        .as_object()
        .ok_or_else(|| invalid(format!("{at} must be an object")))?;
    let Some(raw_actions) = object.get("actions") else {
        return Ok(vec![parse_action(json, event, types, routes, at)?]);
    };
    if object.contains_key("op") {
        return Err(invalid(format!(
            "{at}: actions wrapper must not also contain op"
        )));
    }
    let actions = raw_actions
        .as_array()
        .ok_or_else(|| invalid(format!("{at}.actions must be an array")))?;
    actions
        .iter()
        .enumerate()
        .map(|(i, action)| {
            parse_action(action, event, types, routes, &format!("{at}.actions[{i}]"))
        })
        .collect()
}

fn parse_node(
    json: &Json,
    types: &HashMap<String, Value>,
    routes: Option<(&[String], &str)>,
    ids: &mut HashSet<u32>,
    at: &str,
) -> Result<Node, CoreError> {
    let object = json
        .as_object()
        .ok_or_else(|| invalid(format!("{at} must be an object")))?;
    let raw_id = field(object, "id", at)?
        .as_u64()
        .ok_or_else(|| invalid(format!("{at}.id must be a non-negative integer")))?;
    let id = u32::try_from(raw_id).map_err(|_| invalid(format!("{at}.id {raw_id} exceeds u32")))?;
    if !ids.insert(id) {
        return Err(invalid(format!("duplicate node id {id} at {at}")));
    }
    let kind = string_field(object, "kind", at)?;
    let props = field(object, "props", at)?
        .as_object()
        .cloned()
        .ok_or_else(|| invalid(format!("{at}.props must be an object")))?;
    let children_json = field(object, "children", at)?
        .as_array()
        .ok_or_else(|| invalid(format!("{at}.children must be an array")))?;
    let mut children = Vec::with_capacity(children_json.len());
    for (i, child) in children_json.iter().enumerate() {
        children.push(parse_node(
            child,
            types,
            routes,
            ids,
            &format!("{at}.children[{i}]"),
        )?);
    }
    let when = match object.get("when") {
        None => None,
        Some(Json::String(value)) => match types.get(value) {
            Some(Value::Bool(_)) => Some(value.clone()),
            Some(declared) => {
                return Err(invalid(format!(
                    "{at}.when {value:?} must name a bool state, found {}",
                    declared.type_name()
                )))
            }
            None => {
                return Err(invalid(format!(
                    "{at}.when {value:?} is not declared in state"
                )))
            }
        },
        Some(_) => return Err(invalid(format!("{at}.when must be a string"))),
    };
    let events_json = field(object, "events", at)?
        .as_object()
        .ok_or_else(|| invalid(format!("{at}.events must be an object")))?;
    let mut events = HashMap::new();
    for (event, action) in events_json {
        if !EVENT_NAMES.contains(&event.as_str()) {
            return Err(invalid(format!(
                "{at}.events.{event}: unknown event name (expected {})",
                EVENT_NAMES.join("|")
            )));
        }
        events.insert(
            event.clone(),
            parse_actions(
                action,
                event,
                types,
                routes,
                &format!("{at}.events.{event}"),
            )?,
        );
    }
    Ok(Node {
        id,
        kind,
        props,
        children,
        when,
        events,
    })
}

/// BCP-47 primary subtag plus optional 2–8 char subtags: the shape used by
/// `locale.active` and `locale.supported` (`IR_ADDITIONS.md` §15.1).
pub(crate) fn valid_locale_tag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let Some(primary) = parts.next() else {
        return false;
    };
    (2..=3).contains(&primary.len())
        && primary.bytes().all(|byte| byte.is_ascii_lowercase())
        && parts.all(|part| {
            (2..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

fn parse_locale(json: &Json) -> Result<Locale, CoreError> {
    let object = json
        .as_object()
        .ok_or_else(|| invalid("root.locale must be an object"))?;
    let active = string_field(object, "active", "root.locale")?;
    let raw = object
        .get("supported")
        .and_then(Json::as_array)
        .ok_or_else(|| invalid("root.locale.supported must be an array"))?;
    if raw.is_empty() {
        return Err(invalid("root.locale.supported must not be empty"));
    }
    let mut supported = Vec::with_capacity(raw.len());
    for (i, item) in raw.iter().enumerate() {
        let tag = item
            .as_str()
            .ok_or_else(|| invalid(format!("root.locale.supported[{i}] must be a string")))?;
        if !valid_locale_tag(tag) {
            return Err(invalid(format!(
                "root.locale.supported[{i}] {tag:?} is not a locale tag"
            )));
        }
        supported.push(tag.to_owned());
    }
    if !supported.contains(&active) {
        return Err(invalid(format!(
            "root.locale.active {active:?} is not in supported"
        )));
    }
    Ok(Locale { active, supported })
}

impl Ir {
    pub fn parse(json: &str) -> Result<Ir, CoreError> {
        let root: Json =
            serde_json::from_str(json).map_err(|e| CoreError::InvalidJson(e.to_string()))?;
        let object = root
            .as_object()
            .ok_or_else(|| invalid("root must be a JSON object"))?;
        let version = match field(object, "ir", "root")? {
            Json::String(v) if v == IR_VERSION => v.clone(),
            Json::String(v) => {
                return Err(invalid(format!(
                    "unsupported IR version {v:?}; expected {IR_VERSION:?}"
                )))
            }
            _ => {
                return Err(invalid(format!(
                    "root.ir must be the string {IR_VERSION:?}"
                )))
            }
        };
        let app = string_field(object, "app", "root")?;
        let state_json = field(object, "state", "root")?
            .as_array()
            .ok_or_else(|| invalid("root.state must be an array"))?;
        let (state, types) = parse_state(state_json)?;
        // A multi-page program carries `routes`; a single-page program carries
        // one `tree`.
        // `pages` on a `0.1.0` program is the legacy flat route-name list.
        let (pages, tree, routes, initial_route) = if let Some(Json::Object(map)) =
            object.get("routes")
        {
            if object.contains_key("tree") {
                return Err(invalid(
                    "root.tree is illegal alongside root.routes in a multi-page program",
                ));
            }
            if object.contains_key("pages") {
                return Err(invalid(
                    "root.pages is illegal alongside root.routes; routes supersedes it",
                ));
            }
            let mut ids = HashSet::new();
            // A push target may name any declared route, including one that
            // sorts after the route being parsed. Validate against the full
            // name set, collected before any tree is parsed.
            let all_route_names: Vec<String> = map.keys().cloned().collect();
            if all_route_names.iter().any(|name| name.is_empty()) {
                return Err(invalid("root.routes keys must not be empty"));
            }
            let mut routes = Vec::with_capacity(map.len());
            for (name, entry) in map {
                let at = format!("root.routes.{name}");
                let route_obj = entry
                    .as_object()
                    .ok_or_else(|| invalid(format!("{at} must be an object")))?;
                let title = match route_obj.get("title") {
                    None => None,
                    Some(Json::String(title)) => Some(title.clone()),
                    Some(_) => return Err(invalid(format!("{at}.title must be a string"))),
                };
                let tree = parse_node(
                    field(route_obj, "tree", &at)?,
                    &types,
                    Some((all_route_names.as_slice(), "route")),
                    &mut ids,
                    &format!("{at}.tree"),
                )?;
                routes.push(Route {
                    name: name.clone(),
                    tree,
                    title,
                });
            }
            let initial_route = match object.get("initialRoute") {
                None => {
                    return Err(invalid(
                        "root.initialRoute is required when root.routes is present",
                    ))
                }
                Some(Json::String(name)) => name.clone(),
                Some(_) => return Err(invalid("root.initialRoute must be a string")),
            };
            if !all_route_names.iter().any(|name| name == &initial_route) {
                return Err(invalid(format!(
                    "root.initialRoute {initial_route:?} is not a declared route"
                )));
            }
            (all_route_names, Node::empty(), routes, Some(initial_route))
        } else {
            if object.contains_key("initialRoute") {
                return Err(invalid(
                    "root.initialRoute requires root.routes (multi-page program)",
                ));
            }
            let pages = match object.get("pages") {
                None => Vec::new(),
                Some(Json::Array(list)) => {
                    let mut names = Vec::with_capacity(list.len());
                    for (i, entry) in list.iter().enumerate() {
                        let name = entry
                            .as_str()
                            .ok_or_else(|| invalid(format!("root.pages[{i}] must be a string")))?;
                        if names.iter().any(|existing| existing == name) {
                            return Err(invalid(format!("root.pages[{i}] duplicates {name:?}")));
                        }
                        names.push(name.to_string());
                    }
                    names
                }
                Some(_) => return Err(invalid("root.pages must be an array of strings")),
            };
            let tree = parse_node(
                field(object, "tree", "root")?,
                &types,
                Some((pages.as_slice(), "page")),
                &mut HashSet::new(),
                "root.tree",
            )?;
            (pages, tree, Vec::new(), None)
        };
        let theme = match object.get("theme") {
            None => Json::Null,
            Some(json @ Json::Object(_)) => {
                if let Some(mode) = json.get("mode") {
                    let valid = matches!(mode.as_str(), Some("light" | "dark" | "system"));
                    if !valid {
                        return Err(invalid(
                            "root.theme.mode must be light, dark or system".to_string(),
                        ));
                    }
                }
                if let Some(accent) = json.get("accent") {
                    let text = accent.as_str().unwrap_or("");
                    let valid = text.len() == 7
                        && text.starts_with('#')
                        && text[1..].bytes().all(|b| b.is_ascii_hexdigit());
                    if !valid {
                        return Err(invalid(format!(
                            "root.theme.accent must be #RRGGBB, got {accent}"
                        )));
                    }
                }
                if let Some(radius) = json.get("radius") {
                    if radius.as_f64().is_none_or(|n| !n.is_finite() || n < 0.0) {
                        return Err(invalid(
                            "root.theme.radius must be a non-negative number".to_string(),
                        ));
                    }
                }
                if let Some(font_scale) = json.get("fontScale") {
                    let valid = font_scale
                        .as_f64()
                        .is_some_and(|n| n.is_finite() && (0.5..=3.0).contains(&n));
                    if !valid {
                        return Err(invalid(
                            "root.theme.fontScale must be a number in 0.5..3.0".to_string(),
                        ));
                    }
                }
                if let Some(tokens) = json.get("tokens") {
                    let Some(tokens) = tokens.as_object() else {
                        return Err(invalid("root.theme.tokens must be an object".to_string()));
                    };
                    for key in ["surface", "onSurface", "stage"] {
                        if let Some(value) = tokens.get(key) {
                            let text = value.as_str().unwrap_or("");
                            let valid = text.len() == 7
                                && text.starts_with('#')
                                && text[1..].bytes().all(|b| b.is_ascii_hexdigit());
                            if !valid {
                                return Err(invalid(format!(
                                    "root.theme.tokens.{key} must be #RRGGBB, got {value}"
                                )));
                            }
                        }
                    }
                    if let Some(font) = tokens.get("font") {
                        if font.as_str().is_none() {
                            return Err(invalid(
                                "root.theme.tokens.font must be a string".to_string(),
                            ));
                        }
                    }
                    if let Some(contrast) = tokens.get("contrast") {
                        if !matches!(contrast.as_str(), None | Some("normal") | Some("high")) {
                            return Err(invalid(
                                "root.theme.tokens.contrast must be normal or high".to_string(),
                            ));
                        }
                    }
                }
                json.clone()
            }
            Some(_) => return Err(invalid("root.theme must be an object")),
        };
        let locale = match object.get("locale") {
            None => None,
            Some(json) => Some(parse_locale(json)?),
        };
        Ok(Ir {
            version,
            app,
            state,
            pages,
            tree,
            routes,
            initial_route,
            theme,
            locale,
        })
    }
}
