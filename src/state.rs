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
    version: u64,
    tree: Node,
    hook: Option<Arc<dyn RenderingHook>>,
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
        let result = invoke(capability, params).map_err(CoreError::Plugin)?;
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
}
