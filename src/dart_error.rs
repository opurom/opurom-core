//! Deterministic error taxonomy and stack traces ported from Dart 3.14.0
//! (`dart:core`).
//!
//! Base checkout: `flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb` (`3.50.0`), Dart SDK `3.14.0`
//! revision `fa8c1270c374c1346931583a17c3b7f278d3822d`. Source paths and
//! line numbers are relative to `bin/cache/dart-sdk/lib/`.
//!
//! Ported rules:
//! - the core error classes keep their Dart names: `AssertionError`,
//!   `TypeError`, `ArgumentError`, `RangeError`, `IndexError`,
//!   `NoSuchMethodError`, `UnsupportedError`, `UnimplementedError`,
//!   `StateError`, `ConcurrentModificationError`, `OutOfMemoryError` and
//!   `StackOverflowError` (`core/errors.dart:128-661`);
//! - `AssertionError.toString` is `Assertion failed` with an optional
//!   `: <message>` suffix (`core/errors.dart:135-140`);
//! - `ArgumentError.toString` composes `Invalid argument(s)` / `Invalid
//!   argument (name)` with an optional message and the invalid value
//!   (`core/errors.dart:223-238`);
//! - `RangeError.range` uses the message `Invalid value` by default
//!   (`core/errors.dart:280-288`) and `IndexError` uses `Index out of range`
//!   with the indexable length (`core/errors.dart:445-475`);
//! - `StateError`, `UnsupportedError`, `UnimplementedError` and
//!   `ConcurrentModificationError` use their exact Dart prefixes
//!   (`core/errors.dart:619-643,580-603`);
//! - a stack trace is an immutable string; `StackTrace.empty` is `""` and
//!   `StackTrace.fromString` wraps any string (`core/stacktrace.dart:14-56`).
//!
//! Opurom adaptation: errors are plain data with a deterministic string form
//! and a caller-supplied trace; no runtime reflection or clock is used.

/// A Dart-shaped core error value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DartError {
    /// `AssertionError` (`core/errors.dart:128-141`).
    Assertion { message: Option<String> },
    /// `TypeError` (`core/errors.dart:144`).
    Type,
    /// `ArgumentError.value` with an optional parameter name and message
    /// (`core/errors.dart:163-238`).
    Argument {
        name: Option<String>,
        message: Option<String>,
        invalid_value: Option<String>,
    },
    /// `RangeError.range` with inclusive bounds
    /// (`core/errors.dart:242-288`).
    Range {
        invalid_value: String,
        start: Option<i64>,
        end: Option<i64>,
        name: Option<String>,
        message: Option<String>,
    },
    /// `IndexError` (`core/errors.dart:445-475`).
    Index {
        index: i64,
        length: i64,
        name: Option<String>,
    },
    /// `NoSuchMethodError` (`core/errors.dart:557`).
    NoSuchMethod(String),
    /// `UnsupportedError` (`core/errors.dart:580-585`).
    Unsupported(String),
    /// `UnimplementedError` (`core/errors.dart:595-603`).
    Unimplemented(Option<String>),
    /// `StateError` (`core/errors.dart:619-624`).
    State(String),
    /// `FormatException` (`core/exceptions.dart:40-165`): message, optional
    /// source and zero-based offset.
    FormatException {
        message: String,
        source: Option<String>,
        offset: Option<i64>,
    },
    /// `ConcurrentModificationError` (`core/errors.dart:631-643`).
    ConcurrentModification(Option<String>),
    /// `OutOfMemoryError` (`core/errors.dart:646-653`).
    OutOfMemory,
    /// `StackOverflowError` (`core/errors.dart:655-661`).
    StackOverflow,
}

impl DartError {
    /// The exact Dart `toString` form.
    pub fn dart_to_string(&self) -> String {
        match self {
            Self::Assertion { message } => match message {
                Some(message) => format!("Assertion failed: {message}"),
                None => "Assertion failed".to_owned(),
            },
            Self::Type => "TypeError".to_owned(),
            Self::Argument {
                name,
                message,
                invalid_value,
            } => {
                let error_name = if invalid_value.is_some() {
                    "Invalid argument"
                } else {
                    "Invalid argument(s)"
                };
                let mut out = error_name.to_owned();
                if let Some(name) = name {
                    out.push_str(&format!(" ({name})"));
                }
                if let Some(message) = message {
                    out.push_str(&format!(": {message}"));
                }
                if let Some(value) = invalid_value {
                    out.push_str(&format!(": {value}"));
                }
                out
            }
            Self::Range {
                invalid_value,
                start,
                end,
                name,
                message,
            } => {
                let mut out = "RangeError".to_owned();
                if let Some(name) = name {
                    out.push_str(&format!(" ({name})"));
                }
                out.push_str(&format!(
                    ": {}",
                    message.as_deref().unwrap_or("Invalid value")
                ));
                if let (Some(start), Some(end)) = (start, end) {
                    out.push_str(&format!("[{start}..{end}]"));
                }
                out.push_str(&format!(": {invalid_value}"));
                out
            }
            Self::Index {
                index,
                length,
                name,
            } => {
                let name = name.as_deref().unwrap_or("index");
                format!("RangeError ({name}): Index out of range: {index}, length {length}")
            }
            Self::NoSuchMethod(message) => format!("NoSuchMethodError: {message}"),
            Self::Unsupported(message) => format!("Unsupported operation: {message}"),
            Self::Unimplemented(message) => match message {
                Some(message) => format!("UnimplementedError: {message}"),
                None => "UnimplementedError".to_owned(),
            },
            Self::State(message) => format!("Bad state: {message}"),
            Self::FormatException {
                message,
                source,
                offset,
            } => {
                // `core/exceptions.dart:88-161`: always include the message;
                // for a string source with a valid offset, mark the exact
                // character with a caret and report 1-based line/column.
                let mut report = "FormatException".to_owned();
                if !message.is_empty() {
                    report.push_str(": ");
                    report.push_str(message);
                }
                if let Some(source) = source {
                    match offset {
                        Some(offset) if *offset >= 0 && *offset <= source.len() as i64 => {
                            let offset = *offset as usize;
                            let mut line_num = 1usize;
                            let mut line_start = 0usize;
                            let mut previous_was_cr = false;
                            for (index, character) in
                                source.char_indices().take_while(|(i, _)| *i < offset)
                            {
                                match character {
                                    '\n' => {
                                        if line_start != index || !previous_was_cr {
                                            line_num += 1;
                                        }
                                        line_start = index + 1;
                                        previous_was_cr = false;
                                    }
                                    '\r' => {
                                        line_num += 1;
                                        line_start = index + 1;
                                        previous_was_cr = true;
                                    }
                                    _ => previous_was_cr = false,
                                }
                            }
                            if line_num > 1 {
                                report.push_str(&format!(
                                    " (at line {line_num}, character {})\n",
                                    offset - line_start + 1
                                ));
                            } else {
                                report.push_str(&format!(" (at character {})\n", offset + 1));
                            }
                            let line_end = source[offset..]
                                .find(['\n', '\r'])
                                .map_or(source.len(), |relative| offset + relative);
                            let mut slice = &source[line_start..line_end];
                            let mut prefix = "";
                            let mut postfix = "";
                            if line_end - line_start > 78 {
                                let index = offset - line_start;
                                if index < 75 {
                                    slice = &slice[..75];
                                    postfix = "...";
                                } else if line_end - offset < 75 {
                                    slice = &slice[slice.len() - 75..];
                                    prefix = "...";
                                } else {
                                    slice = &source[offset - 36..offset + 36];
                                    prefix = "...";
                                    postfix = "...";
                                }
                            }
                            let mark = prefix.len() + offset
                                - line_start
                                - if prefix.is_empty() { 0 } else { 0 };
                            report.push_str(prefix);
                            report.push_str(slice);
                            report.push_str(postfix);
                            report.push('\n');
                            for _ in 0..mark {
                                report.push(' ');
                            }
                            report.push_str("^\n");
                        }
                        _ => {
                            // core/exceptions.dart:101-105: no valid offset.
                            report.push('\n');
                            let mut slice = source.clone();
                            if slice.len() > 78 {
                                slice.truncate(75);
                                slice.push_str("...");
                            }
                            report.push_str(&slice);
                        }
                    }
                } else if let Some(offset) = offset {
                    report.push_str(&format!(" (at offset {offset})"));
                }
                report
            }
            Self::ConcurrentModification(object) => match object {
                Some(object) => {
                    format!("Concurrent modification during iteration: {object}.")
                }
                None => "Concurrent modification during iteration.".to_owned(),
            },
            Self::OutOfMemory => "Out of Memory".to_owned(),
            Self::StackOverflow => "Stack Overflow".to_owned(),
        }
    }
}

impl std::fmt::Display for DartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.dart_to_string())
    }
}

impl std::error::Error for DartError {}

/// An immutable stack trace (`core/stacktrace.dart:14-56`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DartStackTrace(String);

impl DartStackTrace {
    /// `StackTrace.empty` (`core/stacktrace.dart:19`).
    pub fn empty() -> Self {
        Self(String::new())
    }

    /// `StackTrace.fromString` (`core/stacktrace.dart:33`).
    pub fn from_string(source: impl Into<String>) -> Self {
        Self(source.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DartStackTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An exception value carrying its error and stack trace, matching the
/// `(error, stack)` pair Dart passes to catch handlers
/// (`core/stacktrace.dart:35-42`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DartException {
    pub error: DartError,
    pub stack_trace: DartStackTrace,
}

impl DartException {
    pub fn new(error: DartError) -> Self {
        Self {
            error,
            stack_trace: DartStackTrace::empty(),
        }
    }

    pub fn with_stack(error: DartError, stack_trace: DartStackTrace) -> Self {
        Self { error, stack_trace }
    }
}

/// `RangeError.checkValueInInterval` (`core/errors.dart:315-327`).
pub fn check_value_in_interval(value: i64, min: i64, max: i64) -> Result<i64, DartError> {
    if value < min || value > max {
        Err(DartError::Range {
            invalid_value: value.to_string(),
            start: Some(min),
            end: Some(max),
            name: None,
            message: None,
        })
    } else {
        Ok(value)
    }
}

/// `RangeError.checkValidIndex` (`core/errors.dart:329-350`): the valid range
/// is `0 <= index < length`.
pub fn check_valid_index(index: i64, length: i64) -> Result<i64, DartError> {
    if index < 0 || index >= length {
        Err(DartError::Index {
            index,
            length,
            name: None,
        })
    } else {
        Ok(index)
    }
}

/// `ArgumentError.checkNotNull` (`core/errors.dart:214-221`).
pub fn check_not_null<T>(value: Option<T>, name: Option<&str>) -> Result<T, DartError> {
    value.ok_or_else(|| DartError::Argument {
        name: name.map(str::to_owned),
        message: Some("Must not be null".to_owned()),
        invalid_value: None,
    })
}
