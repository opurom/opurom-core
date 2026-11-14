//! Kernel-owned text input state: edits travel through the existing plugin bus
//! and emit existing state patches with scalar indices only.

use crate::plugin::PluginBus;
use crate::state::{Event, Patch, Value};
use crate::text_buffer::{TextBuffer, TextBufferError};
use serde_json::{json, Map, Value as Json};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

pub const TEXT_INPUT_BUS: &str = "textInput";
pub const DEFAULT_TEXT_INPUT_CAPACITY: usize = 4096;
const NO_SELECTION: i64 = -1;
const ENCODE_FAILURE: &str =
    r#"{"error":{"kind":"invalidEvent","reason":"cannot encode response"}}"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputError {
    InvalidEvent(String),
    IndexTooLarge { index: u64 },
    Buffer(TextBufferError),
    Bus(String),
}

impl fmt::Display for TextInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEvent(reason) => write!(f, "invalid text input event: {reason}"),
            Self::IndexTooLarge { index } => {
                write!(f, "text input index {index} does not fit usize")
            }
            Self::Buffer(error) => write!(f, "text buffer error: {error}"),
            Self::Bus(message) => write!(f, "text input bus error: {message}"),
        }
    }
}

impl std::error::Error for TextInputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Buffer(error) => Some(error),
            _ => None,
        }
    }
}

impl From<TextBufferError> for TextInputError {
    fn from(error: TextBufferError) -> Self {
        Self::Buffer(error)
    }
}

fn invalid(reason: &str) -> TextInputError {
    TextInputError::InvalidEvent(reason.to_owned())
}

fn bus_error(reason: &str) -> TextInputError {
    TextInputError::Bus(reason.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TextEdit {
    SetText { text: String },
    Insert { text: String },
    Backspace,
    Delete,
    MoveCursor { index: usize },
    Select { start: usize, end: usize },
    ReplaceSelection { text: String },
    SetSecure { secure: bool },
}

pub struct TextInputState {
    bus: PluginBus,
    buffer: Arc<Mutex<TextBuffer>>,
}

impl fmt::Debug for TextInputState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let buffer = lock(&self.buffer);
        f.debug_struct("TextInputState")
            .field("capacity", &buffer.capacity())
            .field("len", &buffer.len())
            .field("display", &buffer.display())
            .field("cursor", &buffer.cursor())
            .field("selection", &buffer.selection())
            .field("secure", &buffer.secure())
            .finish()
    }
}

impl TextInputState {
    pub fn new(capacity: usize) -> Self {
        Self::with_secure(capacity, false)
    }

    pub fn with_secure(capacity: usize, secure: bool) -> Self {
        Self::from_buffer(TextBuffer::new(capacity), secure)
    }

    pub fn with_text(text: &str, capacity: usize, secure: bool) -> Result<Self, TextBufferError> {
        Ok(Self::from_buffer(
            TextBuffer::with_text(text, capacity)?,
            secure,
        ))
    }

    pub fn bus(&self) -> &PluginBus {
        &self.bus
    }

    pub fn dispatch(&self, event: Event) -> Result<Vec<Patch>, TextInputError> {
        let Event::Input { node_id: _, text } = event else {
            return Err(invalid("expected an input event"));
        };
        self.dispatch_json(&text)
    }

    pub fn dispatch_json(&self, event: &str) -> Result<Vec<Patch>, TextInputError> {
        let response = self
            .bus
            .call(TEXT_INPUT_BUS, event)
            .map_err(|error| TextInputError::Bus(error.to_string()))?;
        Self::patches_from_response(&response)
    }

    pub fn patches_from_response(response: &str) -> Result<Vec<Patch>, TextInputError> {
        parse_response(response)
    }

    fn from_buffer(mut buffer: TextBuffer, secure: bool) -> Self {
        buffer.set_secure(secure);
        let shared = Arc::new(Mutex::new(buffer));
        let bus = PluginBus::new(u32::MAX);
        let handler_buffer = Arc::clone(&shared);
        bus.register(TEXT_INPUT_BUS, move |input| {
            Ok::<String, String>(handle(&handler_buffer, input))
        });
        Self {
            bus,
            buffer: shared,
        }
    }
}

struct Snapshot {
    display: String,
    cursor: i64,
    selection_start: i64,
    selection_end: i64,
}

fn parse_edit(input: &str) -> Result<TextEdit, TextInputError> {
    let value: Json =
        serde_json::from_str(input).map_err(|_| invalid("event must be a JSON object"))?;
    let object = value
        .as_object()
        .ok_or_else(|| invalid("event must be a JSON object"))?;
    let op = object
        .get("op")
        .and_then(Json::as_str)
        .ok_or_else(|| invalid("event.op must be a string"))?;
    match op {
        "set_text" => Ok(TextEdit::SetText {
            text: text_field(object, "text")?,
        }),
        "insert" => Ok(TextEdit::Insert {
            text: text_field(object, "text")?,
        }),
        "backspace" => Ok(TextEdit::Backspace),
        "delete" => Ok(TextEdit::Delete),
        "move_cursor" => Ok(TextEdit::MoveCursor {
            index: usize_field(object, "index")?,
        }),
        "select" => Ok(TextEdit::Select {
            start: usize_field(object, "start")?,
            end: usize_field(object, "end")?,
        }),
        "replace_selection" => Ok(TextEdit::ReplaceSelection {
            text: text_field(object, "text")?,
        }),
        "set_secure" => Ok(TextEdit::SetSecure {
            secure: object
                .get("secure")
                .and_then(Json::as_bool)
                .ok_or_else(|| invalid("event.secure must be a bool"))?,
        }),
        _ => Err(invalid("unknown edit op")),
    }
}

fn text_field(object: &Map<String, Json>, key: &str) -> Result<String, TextInputError> {
    object
        .get(key)
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid("event.text must be a string"))
}

fn usize_field(object: &Map<String, Json>, key: &str) -> Result<usize, TextInputError> {
    let raw = object
        .get(key)
        .and_then(Json::as_u64)
        .ok_or_else(|| invalid("event index must be an unsigned integer"))?;
    usize::try_from(raw).map_err(|_| TextInputError::IndexTooLarge { index: raw })
}

fn apply(buffer: &mut TextBuffer, edit: TextEdit) -> Result<(), TextBufferError> {
    match edit {
        TextEdit::SetText { text } => {
            let end = buffer.len();
            buffer.select(0, end)?;
            buffer.replace_selection(&text)
        }
        TextEdit::Insert { text } => buffer.insert(&text),
        TextEdit::Backspace => buffer.backspace(),
        TextEdit::Delete => buffer.delete(),
        TextEdit::MoveCursor { index } => buffer.move_cursor(index),
        TextEdit::Select { start, end } => buffer.select(start, end),
        TextEdit::ReplaceSelection { text } => buffer.replace_selection(&text),
        TextEdit::SetSecure { secure } => {
            buffer.set_secure(secure);
            Ok(())
        }
    }
}

fn snapshot(buffer: &TextBuffer) -> Result<Snapshot, TextInputError> {
    let (selection_start, selection_end) = match buffer.selection() {
        Some((start, end)) => (index_to_i64(start)?, index_to_i64(end)?),
        None => (NO_SELECTION, NO_SELECTION),
    };
    Ok(Snapshot {
        display: buffer.display(),
        cursor: index_to_i64(buffer.cursor())?,
        selection_start,
        selection_end,
    })
}

fn index_to_i64(index: usize) -> Result<i64, TextInputError> {
    i64::try_from(index).map_err(|_| TextInputError::IndexTooLarge {
        index: index as u64,
    })
}

fn snapshot_json(snapshot: &Snapshot) -> Json {
    json!({
        "display": &snapshot.display,
        "cursor": snapshot.cursor,
        "selectionStart": snapshot.selection_start,
        "selectionEnd": snapshot.selection_end,
    })
}

fn build_patches(snapshot: &Snapshot) -> Vec<Patch> {
    vec![
        Patch {
            name: "@textInput.display".to_owned(),
            value: Value::Str(snapshot.display.clone()),
        },
        Patch {
            name: "@textInput.cursor".to_owned(),
            value: Value::Int(snapshot.cursor),
        },
        Patch {
            name: "@textInput.selectionStart".to_owned(),
            value: Value::Int(snapshot.selection_start),
        },
        Patch {
            name: "@textInput.selectionEnd".to_owned(),
            value: Value::Int(snapshot.selection_end),
        },
    ]
}

fn handle(shared: &Arc<Mutex<TextBuffer>>, input: &str) -> String {
    let response = match parse_edit(input) {
        Err(error) => json!({ "error": error_to_json(&error) }),
        Ok(edit) => {
            let mut buffer = lock(shared);
            let mut next = (*buffer).clone();
            match apply(&mut next, edit) {
                Ok(()) => match snapshot(&next) {
                    Ok(updated) => {
                        *buffer = next;
                        json!({ "ok": snapshot_json(&updated) })
                    }
                    Err(error) => json!({ "error": error_to_json(&error) }),
                },
                Err(error) => json!({ "error": error_to_json(&TextInputError::Buffer(error)) }),
            }
        }
    };
    serde_json::to_string(&response).unwrap_or_else(|_| ENCODE_FAILURE.to_owned())
}

fn error_to_json(error: &TextInputError) -> Json {
    match error {
        TextInputError::InvalidEvent(reason) => {
            json!({ "kind": "invalidEvent", "reason": reason })
        }
        TextInputError::IndexTooLarge { index } => {
            json!({ "kind": "indexTooLarge", "index": index })
        }
        TextInputError::Buffer(error) => {
            json!({ "kind": "buffer", "details": buffer_error_to_json(error) })
        }
        TextInputError::Bus(message) => {
            json!({ "kind": "bus", "message": message })
        }
    }
}

fn buffer_error_to_json(error: &TextBufferError) -> Json {
    match error {
        TextBufferError::CursorOutOfRange { index, len } => {
            json!({ "variant": "cursorOutOfRange", "index": index, "len": len })
        }
        TextBufferError::InvalidRange { start, end } => {
            json!({ "variant": "invalidRange", "start": start, "end": end })
        }
        TextBufferError::RangeOutOfRange { start, end, len } => {
            json!({ "variant": "rangeOutOfRange", "start": start, "end": end, "len": len })
        }
        TextBufferError::NoSelection => json!({ "variant": "noSelection" }),
        TextBufferError::CapacityExceeded {
            len,
            added,
            capacity,
        } => json!({
            "variant": "capacityExceeded",
            "len": len,
            "added": added,
            "capacity": capacity,
        }),
    }
}

fn parse_response(response: &str) -> Result<Vec<Patch>, TextInputError> {
    let value: Json = serde_json::from_str(response)
        .map_err(|_| bus_error("malformed text input bus response"))?;
    if let Some(error) = value.get("error") {
        return Err(parse_error(error));
    }
    let ok = value
        .get("ok")
        .ok_or_else(|| bus_error("text input bus response has no result"))?;
    let display = ok
        .get("display")
        .and_then(Json::as_str)
        .ok_or_else(|| bus_error("text input display must be a string"))?;
    let cursor = i64_field(ok, "cursor")?;
    let selection_start = i64_field(ok, "selectionStart")?;
    let selection_end = i64_field(ok, "selectionEnd")?;
    if cursor < 0 || selection_start < NO_SELECTION || selection_end < NO_SELECTION {
        return Err(bus_error("text input indices must not be negative"));
    }
    Ok(build_patches(&Snapshot {
        display: display.to_owned(),
        cursor,
        selection_start,
        selection_end,
    }))
}

fn i64_field(value: &Json, key: &str) -> Result<i64, TextInputError> {
    value
        .get(key)
        .and_then(Json::as_i64)
        .ok_or_else(|| bus_error("text input indices must be integers"))
}

fn parse_error(value: &Json) -> TextInputError {
    let Some(kind) = value.get("kind").and_then(Json::as_str) else {
        return bus_error("malformed text input error");
    };
    match kind {
        "invalidEvent" => value
            .get("reason")
            .and_then(Json::as_str)
            .map(str::to_owned)
            .map(TextInputError::InvalidEvent)
            .unwrap_or_else(|| bus_error("malformed text input error")),
        "indexTooLarge" => value
            .get("index")
            .and_then(Json::as_u64)
            .map(|index| TextInputError::IndexTooLarge { index })
            .unwrap_or_else(|| bus_error("malformed text input error")),
        "buffer" => value
            .get("details")
            .and_then(parse_buffer_error)
            .map(TextInputError::Buffer)
            .unwrap_or_else(|| bus_error("malformed text input error")),
        "bus" => value
            .get("message")
            .and_then(Json::as_str)
            .map(str::to_owned)
            .map(TextInputError::Bus)
            .unwrap_or_else(|| bus_error("malformed text input error")),
        _ => bus_error("unknown text input error"),
    }
}

fn parse_buffer_error(value: &Json) -> Option<TextBufferError> {
    match value.get("variant").and_then(Json::as_str)? {
        "cursorOutOfRange" => Some(TextBufferError::CursorOutOfRange {
            index: usize_from_json(value, "index")?,
            len: usize_from_json(value, "len")?,
        }),
        "invalidRange" => Some(TextBufferError::InvalidRange {
            start: usize_from_json(value, "start")?,
            end: usize_from_json(value, "end")?,
        }),
        "rangeOutOfRange" => Some(TextBufferError::RangeOutOfRange {
            start: usize_from_json(value, "start")?,
            end: usize_from_json(value, "end")?,
            len: usize_from_json(value, "len")?,
        }),
        "noSelection" => Some(TextBufferError::NoSelection),
        "capacityExceeded" => Some(TextBufferError::CapacityExceeded {
            len: usize_from_json(value, "len")?,
            added: usize_from_json(value, "added")?,
            capacity: usize_from_json(value, "capacity")?,
        }),
        _ => None,
    }
}

fn usize_from_json(value: &Json, key: &str) -> Option<usize> {
    value
        .get(key)
        .and_then(Json::as_u64)
        .and_then(|raw| usize::try_from(raw).ok())
}

fn lock(buffer: &Mutex<TextBuffer>) -> MutexGuard<'_, TextBuffer> {
    buffer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
