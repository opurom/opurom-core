use crate::error::CoreError;
use crate::ir::{Action, Ir, Node, Op};
use serde_core::ser::{Serialize, SerializeStruct, Serializer};
use std::collections::HashMap;

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
    actions: HashMap<u32, HashMap<String, Action>>,
    version: u64,
}

fn collect_actions(node: &Node, out: &mut HashMap<u32, HashMap<String, Action>>) {
    out.insert(node.id, node.events.clone());
    for child in &node.children {
        collect_actions(child, out);
    }
}

impl Runtime {
    pub fn from_ir_json(json: &str) -> Result<Runtime, CoreError> {
        let ir = Ir::parse(json)?;
        let mut actions = HashMap::new();
        collect_actions(&ir.tree, &mut actions);
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
        Ok(Runtime {
            app: ir.app,
            values,
            index,
            actions,
            version: 0,
        })
    }

    pub fn app(&self) -> &str {
        &self.app
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn state(&self, name: &str) -> Option<&Value> {
        self.index
            .get(name)
            .map(|&i| &self.values[i].1)
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
        let action = self
            .actions
            .get(&node_id)
            .ok_or(CoreError::UnknownNode(node_id))?
            .get(event_name)
            .cloned()
            .ok_or_else(|| CoreError::UnboundEvent {
                node_id,
                event: event_name.to_owned(),
            })?;
        let position = *self
            .index
            .get(&action.target)
            .ok_or_else(|| CoreError::UnknownState(action.target.clone()))?;
        let current = &self.values[position].1;
        let next = match action.op {
            Op::Set => match action.value {
                Some(value) => value,
                None => match text {
                    Some(text) => Value::Str(text),
                    None => {
                        return Err(CoreError::InvalidIr(format!(
                            "set on {} has no value to apply",
                            action.target
                        )))
                    }
                },
            },
            Op::Add => match (current, action.value) {
                (Value::Int(current), Some(Value::Int(delta))) => Value::Int(
                    current
                        .checked_add(delta)
                        .ok_or_else(|| CoreError::Overflow {
                            name: action.target.clone(),
                        })?,
                ),
                _ => {
                    return Err(CoreError::TypeMismatch {
                        name: action.target.clone(),
                        expected: "int",
                    })
                }
            },
            Op::Toggle => match current {
                Value::Bool(current) => Value::Bool(!current),
                _ => {
                    return Err(CoreError::TypeMismatch {
                        name: action.target.clone(),
                        expected: "bool",
                    })
                }
            },
        };
        let type_matches = matches!(
            (current, &next),
            (Value::Int(_), Value::Int(_))
                | (Value::Bool(_), Value::Bool(_))
                | (Value::Str(_), Value::Str(_))
        );
        if !type_matches {
            return Err(CoreError::TypeMismatch {
                name: action.target.clone(),
                expected: current.type_name(),
            });
        }
        self.values[position].1 = next.clone();
        self.version += 1;
        Ok(vec![Patch {
            name: action.target,
            value: next,
        }])
    }
}
