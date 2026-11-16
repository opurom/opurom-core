use crate::error::{CoreError, PluginError};
use crate::ir::{valid_locale_tag, Action, Ir, Locale, Node, Op, Route};
use crate::parallel_sched::Clock;
use crate::render::RenderingHook;
use crate::resource::{ResourceError, ResourceManager};
use crate::text_input::{TextInputState, DEFAULT_TEXT_INPUT_CAPACITY};
use serde_core::ser::{Serialize, SerializeStruct, Serializer};
use serde_json::Value as Json;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub const DEFAULT_RESOURCE_BUDGET: usize = 64 * 1024;

/// Ring caps for the inspector channels (`INSPECTOR_DATA_PLANE.md`).
pub const FRAME_RING: usize = 200;
pub const NETWORK_RING: usize = 200;
pub const MEMORY_HISTORY_RING: usize = 50;

/// The nominal 60 fps budget the kernel records for its own render calls.
/// A host that owns a frame-budget machine passes its real target through
/// `record_frame` instead.
pub const DEFAULT_FRAME_BUDGET_MS: i64 = 16;

/// One frame record: the host frame loop hands the kernel the measured frame
/// plus its frame-budget machine snapshot; the kernel assigns `seq` and keeps
/// the last `FRAME_RING` records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRecord {
    pub seq: u64,
    pub start_ms: i64,
    pub frame_ms: i64,
    pub budget_ms: i64,
    pub tier: String,
    pub nodes: u32,
    pub effects: u32,
}

/// The frame-budget machine output (`WINDOW_FRAMES` p50/p95 nearest-rank,
/// `fps_x10`, tier), computed by the render side; the kernel only holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameReport {
    pub p50_ms: i64,
    pub p95_ms: i64,
    pub fps_x10: i64,
    pub tier: String,
}

/// Host-supplied frame sample: the measured frame and the report snapshot
/// taken from the frame budget state machine after it consumed the frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSample {
    pub start_ms: i64,
    pub frame_ms: i64,
    pub budget_ms: i64,
    pub nodes: u32,
    pub effects: u32,
    pub report: FrameReport,
}

/// Type-erased host clock with a `Debug` impl so `Runtime` keeps deriving it.
struct ClockBox(Arc<dyn Clock>);

impl std::fmt::Debug for ClockBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClockBox")
    }
}

impl ClockBox {
    fn now(&self) -> u64 {
        self.0.now()
    }
}

/// Deterministic default monotonic clock: one logical tick per read, so
/// timings stay reproducible without a wall clock. Hosts inject a real
/// monotonic clock with `Runtime::with_clock`.
#[derive(Debug, Default)]
struct TickClock(Arc<AtomicU64>);

impl Clock for TickClock {
    fn now(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Str(String),
    /// A list state: Phase A stores string elements only, in array order.
    List(Vec<String>),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Int(_) => "int",
            Self::Bool(_) => "bool",
            Self::Str(_) => "string",
            Self::List(_) => "list",
        }
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Int(v) => serializer.serialize_i64(*v),
            Self::Bool(v) => serializer.serialize_bool(*v),
            Self::Str(v) => serializer.serialize_str(v),
            Self::List(items) => serializer.collect_seq(items),
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
    network_next_id: u64,
    frames: VecDeque<FrameRecord>,
    frame_report: Option<FrameReport>,
    frame_seq: u64,
    memory_history: VecDeque<(u64, usize)>,
    image_stats: Option<(u64, u64)>,
    clock: ClockBox,
    locale: Option<Locale>,
    version: u64,
    tree: Node,
    pages: Vec<String>,
    routes: Vec<Route>,
    initial_route: Option<String>,
    text_inputs: HashMap<u32, TextInputState>,
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
    /// Additive inspector fields: a per-record id and the monotonic-clock
    /// timing of the capability call. Present on every new record.
    pub id: u64,
    pub start_ms: i64,
    pub duration_ms: i64,
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

fn text_input_from_props(
    props: &serde_json::Map<String, Json>,
) -> Result<Option<TextInputState>, CoreError> {
    match props.get("textBuffer") {
        None => return Ok(None),
        Some(value) => match value.as_bool() {
            Some(true) => {}
            Some(false) => return Ok(None),
            None => {
                return Err(CoreError::InvalidIr(
                    "node.props.textBuffer must be a bool".to_owned(),
                ))
            }
        },
    }
    let capacity = match props.get("capacity").or_else(|| props.get("maxLength")) {
        Some(value) => value.as_u64().ok_or_else(|| {
            CoreError::InvalidIr("text input capacity must be an unsigned integer".to_owned())
        })?,
        None => DEFAULT_TEXT_INPUT_CAPACITY as u64,
    };
    let capacity = usize::try_from(capacity)
        .map_err(|_| CoreError::InvalidIr("text input capacity does not fit usize".to_owned()))?;
    let secure = match props.get("secure") {
        Some(value) => value
            .as_bool()
            .ok_or_else(|| CoreError::InvalidIr("text input secure must be a bool".to_owned()))?,
        None => false,
    };
    let initial = match props.get("value").or_else(|| props.get("initialValue")) {
        Some(value) => value
            .as_str()
            .ok_or_else(|| CoreError::InvalidIr("text input value must be a string".to_owned()))?,
        None => "",
    };
    TextInputState::with_text(initial, capacity, secure)
        .map(Some)
        .map_err(|error| CoreError::InvalidIr(format!("invalid text input: {error}")))
}

fn collect_text_inputs(
    node: &Node,
    inputs: &mut HashMap<u32, TextInputState>,
) -> Result<(), CoreError> {
    if let Some(state) = text_input_from_props(&node.props)? {
        inputs.insert(node.id, state);
    }
    for child in &node.children {
        collect_text_inputs(child, inputs)?;
    }
    Ok(())
}

fn same_type(left: &Value, right: &Value) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

fn valid_locale_value(value: &Value) -> Result<String, CoreError> {
    let tag = match value {
        Value::Str(tag) => tag.clone(),
        _ => {
            return Err(CoreError::InvalidLocale {
                message: "value must be a string".to_owned(),
            })
        }
    };
    if !valid_locale_tag(&tag) {
        return Err(CoreError::InvalidLocale {
            message: format!("{tag:?} is not a BCP-47 tag"),
        });
    }
    Ok(tag)
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
    // A push is a navigation signal, not a state mutation: the renderer owns
    // the stack, so the kernel emits a reserved patch and leaves state intact.
    if action.op == Op::Push {
        return Ok(Patch {
            name: "@nav".to_string(),
            value: Value::Str(action.target.clone()),
        });
    }
    // A form validation is likewise a reserved signal: the target names the
    // form scope, and the consumer decides what to validate.
    if action.op == Op::Validate {
        return Ok(Patch {
            name: "@validate".to_string(),
            value: Value::Str(action.target.clone()),
        });
    }
    // A pop unwinds the navigation stack; like push, it is a reserved signal
    // the renderer acts on, not a state mutation.
    if action.op == Op::Pop {
        return Ok(Patch {
            name: "@nav".to_string(),
            value: Value::Str("..".to_string()),
        });
    }
    // `set @locale` is the compiler's event spelling of the reserved locale
    // patch (`IR_ADDITIONS.md` §15.4): like `@nav`, it is a signal the kernel
    // emits, never a state write. The dispatch commit applies the switch to
    // the held locale without mirroring it as state.
    if action.op == Op::Set && action.target == "@locale" {
        return Ok(Patch {
            name: "@locale".to_string(),
            value: Value::Str(valid_locale_value(action.value.as_ref().ok_or_else(
                || CoreError::InvalidLocale {
                    message: "value must be a string".to_owned(),
                },
            )?)?),
        });
    }
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
                // An input event with no explicit value injects its text,
                // parsed by the target state's type exactly like the compiled
                // native target: int parses (0 on failure), bool is "true",
                // string is taken as-is.
                let raw = text.ok_or_else(|| {
                    CoreError::InvalidIr(format!("set on {} has no value to apply", action.target))
                })?;
                match &current {
                    Value::Int(_) => Value::Int(raw.parse::<i64>().unwrap_or(0)),
                    Value::Bool(_) => Value::Bool(raw == "true"),
                    Value::Str(_) => Value::Str(raw.to_owned()),
                    // Parse rejects a `set` on a list target, so no input text
                    // can arrive here; report it rather than invent a value.
                    Value::List(_) => {
                        return Err(CoreError::TypeMismatch {
                            name: action.target.clone(),
                            expected: "list",
                        })
                    }
                }
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
        Op::Append => {
            let mut items = match &current {
                Value::List(items) => items.clone(),
                _ => {
                    return Err(CoreError::TypeMismatch {
                        name: action.target.clone(),
                        expected: "list",
                    })
                }
            };
            let item = if let Some(source) = &action.value_from {
                match state_value(values, index, source)? {
                    Value::Str(text) => text,
                    _ => {
                        return Err(CoreError::TypeMismatch {
                            name: source.clone(),
                            expected: "string",
                        })
                    }
                }
            } else if let Some(Value::Str(text)) = &action.value {
                text.clone()
            } else {
                return Err(CoreError::TypeMismatch {
                    name: action.target.clone(),
                    expected: "string",
                });
            };
            items.push(item);
            Value::List(items)
        }
        Op::RemoveAt => {
            let items = match &current {
                Value::List(items) => items,
                _ => {
                    return Err(CoreError::TypeMismatch {
                        name: action.target.clone(),
                        expected: "list",
                    })
                }
            };
            // Parse already rejects a negative or non-integer index; only the
            // live length can push an otherwise valid index out of range, so
            // the upper bound is checked here.
            let index = match action.value {
                Some(Value::Int(index)) => index,
                _ => {
                    return Err(CoreError::InvalidIr(format!(
                        "removeAt on {} needs an integer index",
                        action.target
                    )))
                }
            };
            let position = usize::try_from(index)
                .ok()
                .filter(|position| *position < items.len())
                .ok_or_else(|| CoreError::IndexOutOfBounds {
                    name: action.target.clone(),
                    index,
                    len: items.len(),
                })?;
            let mut items = items.clone();
            items.remove(position);
            Value::List(items)
        }
        Op::Assign => {
            if !matches!(current, Value::List(_)) {
                return Err(CoreError::TypeMismatch {
                    name: action.target.clone(),
                    expected: "list",
                });
            }
            if let Some(source) = &action.value_from {
                match state_value(values, index, source)? {
                    Value::List(items) => Value::List(items),
                    _ => {
                        return Err(CoreError::TypeMismatch {
                            name: source.clone(),
                            expected: "list",
                        })
                    }
                }
            } else if let Some(Value::List(items)) = &action.value {
                Value::List(items.clone())
            } else {
                return Err(CoreError::InvalidIr(format!(
                    "assign on {} has no value to apply",
                    action.target
                )));
            }
        }
        // Signals return before the state lookup above.
        Op::Push | Op::Pop | Op::Validate => {
            unreachable!("signals are handled before state lookup")
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
        // A multi-page program binds events per route tree; a single-page program binds
        // them on its single top-level tree. Node ids are unique across the
        // whole program, so one action map serves both.
        collect_tree(&ir.tree, &[], &mut actions, &mut guards);
        for route in &ir.routes {
            collect_tree(&route.tree, &[], &mut actions, &mut guards);
        }
        let mut text_inputs = HashMap::new();
        collect_text_inputs(&ir.tree, &mut text_inputs)?;
        for route in &ir.routes {
            collect_text_inputs(&route.tree, &mut text_inputs)?;
        }
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
        let pages = ir.pages;
        Ok(Runtime {
            app: ir.app,
            values,
            index,
            actions,
            guards,
            resources: ResourceManager::new(budget),
            network_log: Vec::new(),
            network_next_id: 0,
            frames: VecDeque::new(),
            frame_report: None,
            frame_seq: 0,
            memory_history: VecDeque::new(),
            image_stats: None,
            clock: ClockBox(Arc::new(TickClock::default())),
            locale: ir.locale,
            version: 0,
            tree,
            pages,
            routes: ir.routes,
            initial_route: ir.initial_route,
            text_inputs,
            hook: None,
        })
    }

    /// Registers the rendering extension before the display list is produced.
    pub fn with_rendering_hook(mut self, hook: Arc<dyn RenderingHook>) -> Self {
        self.hook = Some(hook);
        self
    }

    /// The declared locale object, or `None` for an IR without one.
    pub fn locale(&self) -> Option<&Locale> {
        self.locale.as_ref()
    }

    /// Applies one host-supplied patch. `@locale` is a reserved patch like
    /// `@nav` and `@validate`: it never enters the state mirror, but it does
    /// advance the version. An unsupported tag is a typed error and leaves
    /// both the locale and the version untouched.
    pub fn apply_patch(&mut self, patch: &Patch) -> Result<Patch, CoreError> {
        if patch.name != "@locale" {
            return Err(CoreError::InvalidIr(format!(
                "unsupported reserved patch {:?}",
                patch.name
            )));
        }
        let tag = self.validate_locale_switch(&patch.value)?;
        let version = self.next_version()?;
        self.set_active_locale(tag);
        self.version = version;
        Ok(patch.clone())
    }

    /// Validates a locale switch against the declared supported set; both the
    /// host patch command and dispatched events use this one rule.
    fn validate_locale_switch(&self, value: &Value) -> Result<String, CoreError> {
        let tag = valid_locale_value(value)?;
        match &self.locale {
            Some(locale) if locale.supported.contains(&tag) => Ok(tag),
            Some(locale) => Err(CoreError::InvalidLocale {
                message: format!(
                    "{tag:?} is not supported; expected {}",
                    locale.supported.join(", ")
                ),
            }),
            None => Err(CoreError::InvalidLocale {
                message: format!("{tag:?} requires a declared locale"),
            }),
        }
    }

    fn set_active_locale(&mut self, tag: String) {
        if let Some(locale) = &mut self.locale {
            locale.active = tag;
        }
    }

    /// Registers kernel-owned input editing state for a node.
    pub fn with_text_input(mut self, node_id: u32, state: TextInputState) -> Self {
        self.text_inputs.insert(node_id, state);
        self
    }

    /// Injects the host monotonic clock used for inspector timings. Without it
    /// the kernel ticks a logical counter, which keeps timings deterministic.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = ClockBox(clock);
        self
    }

    /// Records the raster host's image cache stats for the memory channel.
    /// Without a raster host the memory report carries `images: null`.
    pub fn with_image_stats(mut self, entries: u64, bytes: u64) -> Self {
        self.image_stats = Some((entries, bytes));
        self
    }

    /// The display list handed to the renderer: the parsed IR tree, after the
    /// registered rendering hook has observed and decorated it. Every call is
    /// one render step, so the kernel records one measured frame per call:
    /// the call duration on the host clock, the nominal budget and the
    /// rendered node/effect counts. The output itself is unchanged.
    pub fn display_list(&mut self) -> Node {
        let start_ms = self.clock.now() as i64;
        let list = self.decorated_list();
        let frame_ms = self.clock.now() as i64 - start_ms;
        let (nodes, effects) = count_render_units(&list);
        self.record_measured_frame(start_ms, frame_ms, nodes, effects);
        list
    }

    /// The decorated tree without the render-step bookkeeping: the pure path
    /// the inspectors and non-render consumers use.
    fn decorated_list(&self) -> Node {
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
        let list = self.decorated_list();
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

    fn hidden(&self, node_id: u32) -> bool {
        self.guards.get(&node_id).is_some_and(|conditions| {
            conditions
                .iter()
                .any(|name| self.state(name) != Some(&Value::Bool(true)))
        })
    }

    fn next_version(&self) -> Result<u64, CoreError> {
        self.version
            .checked_add(1)
            .ok_or_else(|| CoreError::Overflow {
                name: "version".to_owned(),
            })
    }

    /// Routes an input event to the text buffer owned by `node_id`. The event is
    /// left untouched for any other node: an unregistered node reports
    /// `UnknownNode`, and a registered node that owns no buffer reports
    /// `NoTextBufferInput`, without disturbing any buffer or the version.
    pub fn dispatch_to(&mut self, node_id: u32, event: Event) -> Result<Vec<Patch>, CoreError> {
        if !matches!(event, Event::Input { .. }) {
            return Err(CoreError::InvalidIr(
                "dispatch_to expects an input event".to_owned(),
            ));
        }
        if !self.actions.contains_key(&node_id) {
            return Err(CoreError::UnknownNode(node_id));
        }
        if !self.text_inputs.contains_key(&node_id) {
            return Err(CoreError::NoTextBufferInput(node_id));
        }
        self.dispatch(event)
    }

    pub fn dispatch(&mut self, event: Event) -> Result<Vec<Patch>, CoreError> {
        if let Event::Input { node_id, text } = &event {
            if self.text_inputs.contains_key(node_id) {
                if self.hidden(*node_id) {
                    return Ok(Vec::new());
                }
                let version = self.next_version()?;
                let patches = self
                    .text_inputs
                    .get(node_id)
                    .ok_or_else(|| {
                        CoreError::InvalidIr("text input registration changed".to_owned())
                    })?
                    // Patches are named for this node so a UI binds each field.
                    .dispatch_for(*node_id, text)
                    .map_err(CoreError::TextInput)?;
                self.version = version;
                self.record_memory_history();
                return Ok(patches);
            }
        }
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
        if self.hidden(node_id) {
            return Ok(Vec::new());
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
        // `@locale` is a reserved signal: validate it before the commit, then
        // switch the held locale atomically with the state/version commit. It
        // never enters `working`, so it cannot appear in the state mirror.
        let locale_tags: Vec<String> = patches
            .iter()
            .filter(|patch| patch.name == "@locale")
            .map(|patch| self.validate_locale_switch(&patch.value))
            .collect::<Result<_, _>>()?;
        let version = self.next_version()?;
        self.values = working;
        for tag in locale_tags {
            self.set_active_locale(tag);
        }
        self.version = version;
        self.record_memory_history();
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
        // Time the capability call on the host monotonic clock: the start and
        // duration land in the network record as additive integer fields.
        let start_ms = self.clock.now() as i64;
        let result = invoke(capability, params).map_err(CoreError::Plugin)?;
        let duration_ms = self.clock.now() as i64 - start_ms;
        if let Some((method, url, request_bytes)) = network {
            let status = result.get("status").and_then(Json::as_i64).unwrap_or(0);
            let response_bytes = serde_json::to_vec(&result).map(|b| b.len()).unwrap_or(0);
            let id = self.network_next_id;
            self.network_next_id += 1;
            self.network_log.push(NetworkRecord {
                method,
                url,
                status,
                request_bytes,
                response_bytes,
                id,
                start_ms,
                duration_ms,
            });
            // The inspector network channel is a ring, not a ledger.
            if self.network_log.len() > NETWORK_RING {
                self.network_log.remove(0);
            }
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

    /// The declared page names in declaration order. The runtime consumer
    /// supplies each page's UI, so the kernel only exposes the route list.
    pub fn page_records(&self) -> Json {
        // Multi-page programs expose each route with its title; a single-page
        // program exposes its flat page-name list.
        if !self.routes.is_empty() {
            return Json::Array(
                self.routes
                    .iter()
                    .map(|route| {
                        serde_json::json!({
                            "name": route.name,
                            "title": route.title,
                        })
                    })
                    .collect(),
            );
        }
        Json::Array(
            self.pages
                .iter()
                .map(|name| Json::String(name.clone()))
                .collect(),
        )
    }

    /// The initial route of a multi-page program, or `None` for a single-page
    /// program. The renderer seeds its navigation stack with this page.
    pub fn initial_route(&self) -> Option<&str> {
        self.initial_route.as_deref()
    }

    /// The content tree of a declared route, after the rendering hook has
    /// observed and decorated it. Returns `None` for an unknown route or a
    /// single-page program, whose content is `display_list()`. Every call is
    /// one render step of that page, so it records one measured frame whose
    /// node/effect counts cover the page subtree — the symmetric path to
    /// `display_list`, never both for the same render.
    pub fn route_tree(&mut self, name: &str) -> Option<Node> {
        let route = self.routes.iter().find(|route| route.name == name)?;
        let tree = route.tree.clone();
        let start_ms = self.clock.now() as i64;
        let tree = match &self.hook {
            Some(hook) => {
                hook.observe(&tree);
                hook.decorate(&tree)
            }
            None => tree,
        };
        let (nodes, effects) = count_render_units(&tree);
        let frame_ms = self.clock.now() as i64 - start_ms;
        self.record_measured_frame(start_ms, frame_ms, nodes, effects);
        Some(tree)
    }

    /// The network requests captured through the `network.request` capability,
    /// in invocation order, as deterministic JSON records. The inspector
    /// timing fields (`id`, `startMs`, `durationMs`) are additive.
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
                        "id": record.id,
                        "startMs": record.start_ms,
                        "durationMs": record.duration_ms,
                    })
                })
                .collect(),
        )
    }

    /// The memory channel report (`INSPECTOR_DATA_PLANE.md`): the resource
    /// cache ledger, the JSON sizes of the state snapshot and layer records,
    /// the raster host's image stats (`null` where none exists — absent data
    /// is never a fabricated `0`) and the per-dispatch history ring.
    pub fn memory_report(&self) -> Json {
        let images = self
            .image_stats
            .map(|(entries, bytes)| serde_json::json!({"entries": entries, "bytes": bytes}));
        serde_json::json!({
            "cache": {
                "budget": self.resources.budget(),
                "usedBytes": self.resources.used_bytes(),
                "entries": self.resources.len(),
            },
            "stateBytes": self.state_bytes(),
            "layersBytes": self.layers_bytes(),
            "images": images,
            "history": self
                .memory_history
                .iter()
                .map(|(version, total_bytes)| {
                    serde_json::json!({"version": version, "totalBytes": total_bytes})
                })
                .collect::<Vec<_>>(),
            "availability": "full",
            "reason": "",
        })
    }

    /// Accepts one frame from the host frame loop: the kernel assigns `seq`,
    /// keeps the last `FRAME_RING` records and holds the frame-budget report.
    pub fn record_frame(&mut self, sample: FrameSample) {
        let seq = self.frame_seq;
        self.frame_seq += 1;
        self.frames.push_back(FrameRecord {
            seq,
            start_ms: sample.start_ms,
            frame_ms: sample.frame_ms,
            budget_ms: sample.budget_ms,
            tier: sample.report.tier.clone(),
            nodes: sample.nodes,
            effects: sample.effects,
        });
        if self.frames.len() > FRAME_RING {
            self.frames.pop_front();
        }
        self.frame_report = Some(sample.report);
    }

    /// A kernel-measured render step: the same ring, but no frame-budget
    /// report — that machine is host-owned, so the channel stays `partial`
    /// until a host supplies one rather than fabricating a tier.
    fn record_measured_frame(&mut self, start_ms: i64, frame_ms: i64, nodes: u32, effects: u32) {
        let seq = self.frame_seq;
        self.frame_seq += 1;
        self.frames.push_back(FrameRecord {
            seq,
            start_ms,
            frame_ms,
            budget_ms: DEFAULT_FRAME_BUDGET_MS,
            // The tier comes from the host frame-budget machine; a
            // kernel-measured frame has none yet, so the field stays empty.
            tier: String::new(),
            nodes,
            effects,
        });
        if self.frames.len() > FRAME_RING {
            self.frames.pop_front();
        }
    }

    /// Frame records after `since_seq` (paging: strictly greater), or every
    /// held record when no sequence number is given.
    pub fn frame_records(&self, since_seq: Option<u64>) -> Vec<FrameRecord> {
        self.frames
            .iter()
            .filter(|record| since_seq.is_none_or(|since| record.seq > since))
            .cloned()
            .collect()
    }

    /// The latest frame-budget machine snapshot the host passed in.
    pub fn frame_report(&self) -> Option<&FrameReport> {
        self.frame_report.as_ref()
    }

    /// The frames channel response for the `frames [sinceSeq]` protocol
    /// command: records, the held report and channel availability. With no
    /// recorded frame the channel is `absent`; kernel-measured frames without
    /// a host frame-budget report are `partial` — never a fabricated tier.
    pub fn frames_channel(&self, since_seq: Option<u64>) -> Json {
        let (availability, reason) = match (&self.frame_report, self.frames.is_empty()) {
            (_, true) => ("absent", "no frame records yet"),
            (Some(_), false) => ("full", ""),
            (None, false) => ("partial", "no frame-budget report from the host"),
        };
        serde_json::json!({
            "frames": self
                .frame_records(since_seq)
                .iter()
                .map(frame_record_json)
                .collect::<Vec<_>>(),
            "report": self.frame_report.as_ref().map(|report| {
                serde_json::json!({
                    "p50Ms": report.p50_ms,
                    "p95Ms": report.p95_ms,
                    "fpsX10": report.fps_x10,
                    "tier": report.tier,
                })
            }),
            "availability": availability,
            "reason": reason,
        })
    }

    /// One `{version, totalBytes}` memory-history entry per dispatch that
    /// advanced the version, capped at the ring size.
    fn record_memory_history(&mut self) {
        let total = self.state_bytes() + self.layers_bytes();
        self.memory_history.push_back((self.version, total));
        if self.memory_history.len() > MEMORY_HISTORY_RING {
            self.memory_history.pop_front();
        }
    }

    fn state_bytes(&self) -> usize {
        serde_json::to_string(&self.snapshot())
            .map(|json| json.len())
            .unwrap_or(0)
    }

    fn layers_bytes(&self) -> usize {
        serde_json::to_string(&self.layer_records())
            .map(|json| json.len())
            .unwrap_or(0)
    }
}

/// The rendered node count and the total effect-layer count of a display
/// list, the per-frame units the frame record carries.
fn count_render_units(node: &Node) -> (u32, u32) {
    let mut nodes = 1u32;
    let mut effects = effect_layers(node).as_array().map(Vec::len).unwrap_or(0) as u32;
    for child in &node.children {
        let (child_nodes, child_effects) = count_render_units(child);
        nodes += child_nodes;
        effects += child_effects;
    }
    (nodes, effects)
}

/// The frame record JSON shape from `INSPECTOR_DATA_PLANE.md`. `phaseMs` is
/// the v1 `null`: the layout/raster split ships in P2.
fn frame_record_json(record: &FrameRecord) -> Json {
    serde_json::json!({
        "seq": record.seq,
        "startMs": record.start_ms,
        "frameMs": record.frame_ms,
        "budgetMs": record.budget_ms,
        "phaseMs": Json::Null,
        "tier": record.tier,
        "nodes": record.nodes,
        "effects": record.effects,
    })
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
            "ir": "0.1.0", "app": "T",
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
