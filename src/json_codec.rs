//! Deterministic JSON codec for the core value tree, ported from Dart 3.14.0
//! (`dart:convert`).
//!
//! Base checkout: `flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb` (`3.50.0`), Dart SDK `3.14.0`
//! revision `fa8c1270c374c1346931583a17c3b7f278d3822d`. Source paths and
//! line numbers are relative to `bin/cache/dart-sdk/lib/`.
//!
//! Ported rules:
//! - the directly encodable values are `num`, `String`, `bool`, `Null`,
//!   `List` and `Map` with string keys (`convert/json.dart:70-88,292-302,
//!   325-330`);
//! - `jsonEncode` emits a single-line string by default; indentation is
//!   opt-in via `JsonEncoder.withIndent` (`:277-323`);
//! - `jsonDecode` returns numbers as Dart `num` values: an integer-looking
//!   token decodes to `int`, a fractional/exponent token to `double`
//!   (`:157-160`, `:63-65`);
//! - an unencodable value is a typed error in this kernel: the runtime has no
//!   reflection and no `.toJson()` hook (`:72-78`);
//! - object key order is preserved from the source `FutureValue::Map`, giving
//!   byte-identical output on every host (deterministic rendering red line).
//!
//! Opurom adaptation: values are `FutureValue`, so a decoded JSON tree can be
//! handed directly to the existing IR/state layer without an interpreter.

use serde_json::Value as Json;

use crate::future::FutureValue;

/// Error returned by the JSON codec.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonError {
    /// Invalid JSON text.
    InvalidJson(String),
    /// The value cannot be encoded without reflection/toJson.
    Unencodable(String),
    /// A float that has no finite representation.
    NonFiniteFloat,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(reason) => write!(f, "invalid JSON: {reason}"),
            Self::Unencodable(reason) => write!(f, "unencodable value: {reason}"),
            Self::NonFiniteFloat => f.write_str("non-finite float"),
        }
    }
}

impl std::error::Error for JsonError {}

/// Encodes a `FutureValue` as compact JSON (`convert/json.dart:277-302`).
/// Object keys keep source order, matching Dart `Map` iteration and the
/// deterministic rendering red line.
pub fn json_encode(value: &FutureValue) -> Result<String, JsonError> {
    let mut out = String::new();
    write_json(value, None, 0, &mut out)?;
    Ok(out)
}

/// Encodes a `FutureValue` as pretty JSON with the caller's indent string
/// (`convert/json.dart:304-323`).
pub fn json_encode_pretty(value: &FutureValue, indent: &str) -> Result<String, JsonError> {
    let mut out = String::new();
    write_json(value, Some(indent), 0, &mut out)?;
    Ok(out)
}

/// Decodes JSON text into a `FutureValue` (`convert/json.dart:157-160`).
pub fn json_decode(source: &str) -> Result<FutureValue, JsonError> {
    let json: Json =
        serde_json::from_str(source).map_err(|error| JsonError::InvalidJson(error.to_string()))?;
    from_json(&json)
}

/// Writes one value; `indent == None` gives compact output.
fn write_json(
    value: &FutureValue,
    indent: Option<&str>,
    level: usize,
    out: &mut String,
) -> Result<(), JsonError> {
    match value {
        FutureValue::Null => out.push_str("null"),
        FutureValue::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        FutureValue::Int(value) => out.push_str(&value.to_string()),
        FutureValue::Float(value) => {
            if !value.is_finite() {
                return Err(JsonError::NonFiniteFloat);
            }
            out.push_str(&value.to_string());
        }
        FutureValue::Str(value) => {
            out.push_str(
                &serde_json::to_string(value)
                    .map_err(|error| JsonError::Unencodable(error.to_string()))?,
            );
        }
        FutureValue::List(values) => {
            if values.is_empty() {
                out.push_str("[]");
                return Ok(());
            }
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline_indent(out, indent, level + 1);
                write_json(value, indent, level + 1, out)?;
            }
            newline_indent(out, indent, level);
            out.push(']');
        }
        FutureValue::Map(entries) => {
            if entries.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            out.push('{');
            for (index, (key, value)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline_indent(out, indent, level + 1);
                out.push_str(
                    &serde_json::to_string(key)
                        .map_err(|error| JsonError::Unencodable(error.to_string()))?,
                );
                if indent.is_some() {
                    out.push_str(": ");
                } else {
                    out.push(':');
                }
                write_json(value, indent, level + 1, out)?;
            }
            newline_indent(out, indent, level);
            out.push('}');
        }
    }
    Ok(())
}

fn newline_indent(out: &mut String, indent: Option<&str>, level: usize) {
    if let Some(indent) = indent {
        out.push('\n');
        for _ in 0..level {
            out.push_str(indent);
        }
    }
}

fn from_json(value: &Json) -> Result<FutureValue, JsonError> {
    match value {
        Json::Null => Ok(FutureValue::Null),
        Json::Bool(value) => Ok(FutureValue::Bool(*value)),
        Json::Number(number) => {
            if let Some(integer) = number.as_i64() {
                Ok(FutureValue::Int(integer))
            } else if let Some(unsigned) = number.as_u64() {
                Ok(FutureValue::Int(unsigned as i64))
            } else {
                number
                    .as_f64()
                    .map(FutureValue::Float)
                    .ok_or_else(|| JsonError::InvalidJson("number out of range".to_owned()))
            }
        }
        Json::String(value) => Ok(FutureValue::Str(value.clone())),
        Json::Array(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(from_json(value)?);
            }
            Ok(FutureValue::List(out))
        }
        Json::Object(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                out.push((key.clone(), from_json(value)?));
            }
            Ok(FutureValue::Map(out))
        }
    }
}
