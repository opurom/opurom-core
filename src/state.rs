use crate::error::{CoreError, PluginError};
use crate::ir::{Action, Ir, Node, Op};
use crate::render::RenderingHook;
use crate::resource::{ResourceError, ResourceManager};
use serde_core::ser::{Serialize, SerializeStruct, Serializer};
use serde_json::Value as Json;
use std::collections::HashMap;
use std::sync::Arc;

pub const DEFAULT_RESOURCE_BUDGET: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Str(String),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Int(_) => "int",
            Self::Bool(_) => "bool",
            Self::Str(_) => "string",
        }
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Int(v) => serializer.serialize_i64(*v),
            Self::Bool(v) => serializer.serialize_bool(*v),
            Self::Str(v) => serializer.serialize_str(v),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub name: String,
    pub value: Value,
}

impl Serialize for Patch {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Patch", 2)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("value", &self.value)?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Tap { node_id: u32 },
    DoubleTap { node_id: u32 },
    LongPress { node_id: u32 },
    Drag { node_id: u32 },
    Input { node_id: u32, text: String },
}

#[derive(Debug)]
pub struct Runtime {
    app: String,
    values: Vec<(String, Value)>,
    index: HashMap<String, usize>,
    actions: HashMap<u32, HashMap<String, Vec<Action>>>,
    guards: HashMap<u32, Vec<String>>,
    resources: ResourceManager<String, Json>,
    network_log: Vec<NetworkRecord>,
    version: u64,
    tree: Node,
    hook: Option<Arc<dyn RenderingHook>>,
}

/// One captured network request: the method and URL the plugin was invoked with,
/// the HTTP status the plugin reported, and the request/response byte counts.
/// Recorded deterministically from the capability call, with no wall clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkRecord {
    pub method: String,
    pub url: String,
    pub status: i64,
    pub request_bytes: usize,
    pub response_bytes: usize,
}

fn collect_tree(
    node: &Node,
    inherited: &[String],
    actions: &mut HashMap<u32, HashMap<String, Vec<Action>>>,
    guards: &mut HashMap<u32, Vec<String>>,
) {
    actions.insert(node.id, node.events.clone());
    let mut chain = inherited.to_vec();
    if let Some(when) = &node.when {
        chain.push(when.clone());
    }
    // An ancestor's condition also hides the whole subtree.
    if !chain.is_empty() {
        guards.insert(node.id, chain.clone());
    }
    for child in &node.children {
        collect_tree(child, &chain, actions, guards);
    }
}

fn same_type(left: &Value, right: &Value) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

fn state_value(
    values: &[(String, Value)],
    index: &HashMap<String, usize>,
    name: &str,
) -> Result<Value, CoreError> {
    let position = *index
        .get(name)
        .ok_or_else(|| CoreError::UnknownState(name.to_owned()))?;
    Ok(values[position].1.clone())
}

fn apply_action(
    action: &Action,
    values: &mut Vec<(String, Value)>,
    index: &HashMap<String, usize>,
    text: Option<&str>,
) -> Result<Patch, CoreError> {
    let position = *index
        .get(&action.target)
        .ok_or_else(|| CoreError::UnknownState(action.target.clone()))?;
    let current = values[position].1.clone();
    if action.value.is_some() && action.value_from.is_some() {
        return Err(CoreError::InvalidIr(format!(
            "action on {} has both value and valueFrom",
            action.target
        )));
    }
    let next = match action.op {
        Op::Set => {
            if let Some(source) = &action.value_from {
                let value = state_value(values, index, source)?;
                if !same_type(&current, &value) {
                    return Err(CoreError::TypeMismatch {
                        name: source.clone(),
                        expected: current.type_name(),
                    });
                }
                value
            } else if let Some(value) = &action.value {
                value.clone()
            } else {
                Value::Str(
                    text.ok_or_else(|| {
                        CoreError::InvalidIr(format!(
                            "set on {} has no value to apply",
                            action.target
                        ))
                    })?
                    .to_owned(),
                )
            }
        }
        Op::Add | Op::Sub => {
            let delta = if let Some(source) = &action.value_from {
                match state_value(values, index, source)? {
                    Value::Int(value) => value,
                    _ => {
                        return Err(CoreError::TypeMismatch {
                            name: source.clone(),
                            expected: "int",
                        })
                    }
                }
            } else if let Some(Value::Int(value)) = action.value {
                value
            } else {
                return Err(CoreError::TypeMismatch {
                    name: action.target.clone(),
                    expected: "int",
                });
            };
            match &current {
                Value::Int(value) => {
                    let next = if action.op == Op::Add {
                        value.checked_add(delta)
                    } else {
                        value.checked_sub(delta)
                    };
                    Value::Int(next.ok_or_else(|| CoreError::Overflow {
                        name: action.target.clone(),
                    })?)
                }
                _ => {
                    return Err(CoreError::TypeMismatch {
                        name: action.target.clone(),
                        expected: "int",
                    })
                }
            }
        }
        Op::Toggle => {
            if action.value.is_some() || action.value_from.is_some() {
                return Err(CoreError::InvalidIr(format!(
                    "toggle on {} must not have a value",
                    action.target
                )));
            }
            match &current {
                Value::Bool(value) => Value::Bool(!value),
                _ => {
                    return Err(CoreError::TypeMismatch {
                        name: action.target.clone(),
                        expected: "bool",
                    })
                }
            }
        }
    };
    if !same_type(&current, &next) {
        return Err(CoreError::TypeMismatch {
            name: action.target.clone(),
            expected: current.type_name(),
        });
    }
    values[position].1 = next.clone();
    Ok(Patch {
        name: action.target.clone(),
        value: next,
    })
}

impl Runtime {
    pub fn from_ir_json(json: &str) -> Result<Runtime, CoreError> {
        Self::from_ir_json_with_budget(json, DEFAULT_RESOURCE_BUDGET)
    }

    pub fn from_ir_json_with_budget(json: &str, budget: usize) -> Result<Runtime, CoreError> {
        let ir = Ir::parse(json)?;
        let mut actions = HashMap::new();
        let mut guards = HashMap::new();
        collect_tree(&ir.tree, &[], &mut actions, &mut guards);
        let values: Vec<(String, Value)> = ir
            .state
            .into_iter()
            .map(|decl| (decl.name, decl.value))
            .collect();
        let index = values
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (name.clone(), i))
            .collect();
        let tree = ir.tree;
        Ok(Runtime {
            app: ir.app,
            values,
            index,
            actions,
            guards,
            resources: ResourceManager::new(budget),
            network_log: Vec::new(),
            version: 0,
            tree,
            hook: None,
        })
    }

    /// Registers the rendering extension before the display list is produced.
    pub fn with_rendering_hook(mut self, hook: Arc<dyn RenderingHook>) -> Self {
        self.hook = Some(hook);
        self
    }

    /// The display list handed to the renderer: the parsed IR tree, after the
    /// registered rendering hook has observed and decorated it. Without a
    /// hook this is the parsed tree unchanged.
    pub fn display_list(&self) -> Node {
        let list = self.tree.clone();
        if let Some(hook) = &self.hook {
            hook.observe(&list);
            hook.decorate(&list)
        } else {
            list
        }
    }

    /// The live display list as deterministic layer records for inspection: one
    /// `{id, kind, effects}` entry per node, in tree order. Effects are derived
    /// from the node props (shadow, glass, gradient, opacity and declared
    /// effects), matching the CLI inspect layer view.
    pub fn layer_records(&self) -> Json {
        let list = self.display_list();
        let mut records = Vec::new();
        collect_layers(&list, &mut records);
        Json::Array(records)
    }

    pub fn app(&self) -> &str {
        &self.app
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn state(&self, name: &str) -> Option<&Value> {
        self.index.get(name).map(|&i| &self.values[i].1)
    }

    pub fn snapshot(&self) -> Vec<Patch> {
        self.values
            .iter()
            .map(|(name, value)| Patch {
                name: name.clone(),
                value: value.clone(),
            })
            .collect()
    }

    pub fn dispatch(&mut self, event: Event) -> Result<Vec<Patch>, CoreError> {
        let (node_id, event_name, text) = match event {
            Event::Tap { node_id } => (node_id, "tap", None),
            Event::DoubleTap { node_id } => (node_id, "doubleTap", None),
            Event::LongPress { node_id } => (node_id, "longPress", None),
            Event::Drag { node_id } => (node_id, "drag", None),
            Event::Input { node_id, text } => (node_id, "input", Some(text)),
        };
        let actions = self
            .actions
            .get(&node_id)
            .ok_or(CoreError::UnknownNode(node_id))?
            .get(event_name)
            .ok_or_else(|| CoreError::UnboundEvent {
                node_id,
                event: event_name.to_owned(),
            })?;
        // A hidden node is inert: a bound event must not mutate state behind
        // a false condition.
        if let Some(conditions) = self.guards.get(&node_id) {
            if conditions
                .iter()
                .any(|name| self.state(name) != Some(&Value::Bool(true)))
            {
                return Ok(Vec::new());
            }
        }
        if actions.is_empty() {
            return Ok(Vec::new());
        }
        let mut working = self.values.clone();
        let mut patches = Vec::with_capacity(actions.len());
        for action in actions {
            patches.push(apply_action(
                action,
                &mut working,
                &self.index,
                text.as_deref(),
            )?);
        }
        let version = self
            .version
            .checked_add(1)
            .ok_or_else(|| CoreError::Overflow {
                name: "version".to_owned(),
            })?;
        self.values = working;
        self.version = version;
        Ok(patches)
    }

    /// Invokes a plugin once per key and caches its JSON result under the runtime byte budget.
    pub fn invoke_plugin<F>(
        &mut self,
        capability: &str,
        params: Json,
        invoke: F,
    ) -> Result<Json, CoreError>
    where
        F: FnOnce(&str, Json) -> Result<Json, PluginError>,
    {
        let key = serde_json::to_string(&serde_json::json!([capability, &params]))
            .map_err(|error| CoreError::Resource(ResourceError::InvalidValue(error.to_string())))?;
        if let Some(cached) = self.resources.get(&key) {
            return Ok(cached);
        }
        // Capture the network request shape before the params are consumed. A
        // `network.request` records its method and URL from the params and its
        // status plus byte counts from the result; nothing else is logged.
        let network = if capability == "network.request" {
            Some((
                params
                    .get("method")
                    .and_then(Json::as_str)
                    .unwrap_or("GET")
                    .to_owned(),
                params
                    .get("url")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_owned(),
                serde_json::to_vec(&params).map(|b| b.len()).unwrap_or(0),
            ))
        } else {
            None
        };
        let result = invoke(capability, params).map_err(CoreError::Plugin)?;
        if let Some((method, url, request_bytes)) = network {
            let status = result.get("status").and_then(Json::as_i64).unwrap_or(0);
            let response_bytes = serde_json::to_vec(&result).map(|b| b.len()).unwrap_or(0);
            self.network_log.push(NetworkRecord {
                method,
                url,
                status,
                request_bytes,
                response_bytes,
            });
        }
        let bytes = serde_json::to_vec(&result)
            .map_err(|error| CoreError::Resource(ResourceError::InvalidValue(error.to_string())))?;
        // A result larger than the whole budget is not cached, but the call still
        // succeeds: the cache is an optimisation, not a contract.
        match self.resources.insert(key, result.clone(), bytes.len()) {
            Ok(_) => Ok(result),
            Err(ResourceError::BudgetExceeded { .. }) => Ok(result),
            Err(error) => Err(CoreError::Resource(error)),
        }
    }

    pub fn resource_cache(&self) -> &ResourceManager<String, Json> {
        &self.resources
    }

    /// The network requests captured through the `network.request` capability,
    /// in invocation order, as deterministic JSON records.
    pub fn network_records(&self) -> Json {
        Json::Array(
            self.network_log
                .iter()
                .map(|record| {
                    serde_json::json!({
                        "method": record.method,
                        "url": record.url,
                        "status": record.status,
                        "requestBytes": record.request_bytes,
                        "responseBytes": record.response_bytes,
                    })
                })
                .collect(),
        )
    }
}

/// Effect layers declared by a node, in a stable order: the built-in visual
/// effects derived from props, then any custom declared effect kinds. Numeric
/// props are read as floats so integer and fractional encodings match the CLI
/// inspect layer view exactly.
fn effect_layers(node: &Node) -> Json {
    let mut kinds: Vec<String> = Vec::new();
    if let Some(shadow) = node.props.get("shadow").and_then(Json::as_f64) {
        if shadow > 0.0 {
            kinds.push("shadow".to_owned());
        }
    }
    if node.props.get("glass").and_then(Json::as_bool) == Some(true) {
        kinds.push("glass".to_owned());
    }
    if let Some(gradient) = node.props.get("gradient").and_then(Json::as_str) {
        if gradient.contains(',') {
            kinds.push("gradient".to_owned());
        }
    }
    if let Some(opacity) = node.props.get("opacity").and_then(Json::as_f64) {
        if opacity < 1.0 {
            kinds.push("opacity".to_owned());
        }
    }
    if let Some(effects) = node.props.get("effects").and_then(Json::as_array) {
        for effect in effects {
            if let Some(kind) = effect.get("kind").and_then(Json::as_str) {
                if !kinds.iter().any(|existing| existing == kind) {
                    kinds.push(kind.to_owned());
                }
            }
        }
    }
    Json::Array(kinds.into_iter().map(Json::String).collect())
}

fn collect_layers(node: &Node, out: &mut Vec<Json>) {
    out.push(serde_json::json!({
        "id": node.id,
        "kind": node.kind,
        "effects": effect_layers(node),
    }));
    for child in &node.children {
        collect_layers(child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_records_derives_effect_layers_from_props() {
        let ir = r##"{
            "ir": "0.1", "app": "T",
            "state": [{"name": "n", "type": "int", "init": 0}],
            "theme": {"mode": "system", "accent": "#097FEB", "radius": 12},
            "tree": {
                "id": 0, "kind": "Column", "props": {}, "children": [
                    {"id": 1, "kind": "Container", "props": {"shadow": 3, "glass": true, "gradient": "#AA,#BB", "opacity": 0.0}, "children": [], "events": {}},
                    {"id": 2, "kind": "Text", "props": {"value": "x"}, "children": [], "events": {}}
                ], "events": {}
            }
        }"##;
        let runtime = Runtime::from_ir_json(ir).expect("IR parses");
        let records = runtime
            .layer_records()
            .as_array()
            .expect("layer records are an array")
            .clone();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["kind"].as_str(), Some("Column"));
        assert_eq!(records[0]["effects"], Json::Array(vec![]));
        assert_eq!(records[1]["id"], 1);
        assert_eq!(
            records[1]["effects"],
            serde_json::json!(["shadow", "glass", "gradient", "opacity"])
        );
        assert_eq!(records[2]["kind"].as_str(), Some("Text"));
        assert_eq!(records[2]["effects"], Json::Array(vec![]));
    }
}
