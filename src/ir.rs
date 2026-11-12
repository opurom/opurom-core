use crate::error::CoreError;
use crate::state::Value;
use serde_json::{Map, Value as Json};
use std::collections::{HashMap, HashSet};

pub const IR_VERSION: &str = "0.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Set,
    Add,
    Toggle,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDecl {
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ir {
    pub version: String,
    pub app: String,
    pub state: Vec<StateDecl>,
    pub tree: Node,
    pub theme: Json,
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
    };
    parsed.ok_or_else(|| CoreError::TypeMismatch {
        name: name.to_owned(),
        expected: declared.type_name(),
    })
}

fn same_declared_type(left: &Value, right: &Value) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
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
            _ => {
                return Err(invalid(format!(
                    "{at}.type {kind:?} is not int, bool or string"
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
            "{at}: add requires an int valueFrom source, {source:?} is {}",
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
    at: &str,
) -> Result<Action, CoreError> {
    let object = json
        .as_object()
        .ok_or_else(|| invalid(format!("{at} must be an object")))?;
    let op = string_field(object, "op", at)?;
    let target = string_field(object, "target", at)?;
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
        "add" => {
            if !matches!(declared, Value::Int(_)) {
                return Err(invalid(format!(
                    "{at}: add requires an int target, {target:?} is {}",
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
                    .ok_or_else(|| invalid(format!("{at}: add requires an integer value")))?;
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
        "set" => match (raw_value, raw_from) {
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
                if !matches!(declared, Value::Str(_)) {
                    return Err(invalid(format!(
                        "{at}: input set without value requires a string target"
                    )));
                }
                value = None;
                value_from = None;
            }
            (Some(_), Some(_)) => {
                return Err(invalid(format!(
                    "{at}: value and valueFrom are mutually exclusive"
                )))
            }
        },
        _ => return Err(invalid(format!("{at}.op {op:?} is not set, add or toggle"))),
    };
    let op = match op.as_str() {
        "set" => Op::Set,
        "add" => Op::Add,
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
    at: &str,
) -> Result<Vec<Action>, CoreError> {
    let object = json
        .as_object()
        .ok_or_else(|| invalid(format!("{at} must be an object")))?;
    let Some(raw_actions) = object.get("actions") else {
        return Ok(vec![parse_action(json, event, types, at)?]);
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
        .map(|(i, action)| parse_action(action, event, types, &format!("{at}.actions[{i}]")))
        .collect()
}

fn parse_node(
    json: &Json,
    types: &HashMap<String, Value>,
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
            ids,
            &format!("{at}.children[{i}]"),
        )?);
    }
    let when = match object.get("when") {
        None => None,
        Some(Json::String(value)) => Some(value.clone()),
        Some(_) => return Err(invalid(format!("{at}.when must be a string"))),
    };
    let events_json = field(object, "events", at)?
        .as_object()
        .ok_or_else(|| invalid(format!("{at}.events must be an object")))?;
    let mut events = HashMap::new();
    for (event, action) in events_json {
        events.insert(
            event.clone(),
            parse_actions(action, event, types, &format!("{at}.events.{event}"))?,
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
        let tree = parse_node(
            field(object, "tree", "root")?,
            &types,
            &mut HashSet::new(),
            "root.tree",
        )?;
        let theme = match object.get("theme") {
            None => Json::Null,
            Some(json @ Json::Object(_)) => json.clone(),
            Some(_) => return Err(invalid("root.theme must be an object")),
        };
        Ok(Ir {
            version,
            app,
            state,
            tree,
            theme,
        })
    }
}
