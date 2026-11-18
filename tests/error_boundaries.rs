//! Linked boundary tests: Dart errors ↔ JSON / collections / conversion.
//!
//! Pinned to Flutter 3.50.0 (`flutter-golden` HEAD
//! ab2912035e160d9cad6306df307576da6128ebdb) and Dart SDK 3.14.0.
//! Sources: `core/exceptions.dart:40-165`, `core/errors.dart:128-661`,
//! `convert/base64.dart:683-761`, `convert/json.dart:592`,
//! `convert/utf.dart:571`, `core/iterable.dart:385-429`.

use opurom_core::{
    collect_where, convert_error_as_format, json_decode, json_encode, json_error_as_format, reduce,
    DartError, JsonError, NoElementError,
};

#[test]
fn json_invalid_input_maps_to_dart_format_exception() {
    // convert/json.dart:592 throws FormatException on invalid text;
    // core/exceptions.dart:88-93 always includes the message.
    let error = json_decode("{").unwrap_err();
    let format = json_error_as_format(&error);
    let rendered = format.dart_to_string();
    assert!(rendered.starts_with("FormatException: "), "{rendered}");
    assert!(matches!(
        format,
        DartError::FormatException {
            source: None,
            offset: None,
            ..
        }
    ));
}

#[test]
fn non_finite_json_number_has_stable_error_string() {
    // Dart numbers are IEEE 754 doubles (core/double.dart:7-12); a non-finite
    // value has no JSON token, so the kernel reports a typed error.
    let error = json_encode(&opurom_core::FutureValue::Float(f64::NAN)).unwrap_err();
    assert_eq!(error, JsonError::NonFiniteFloat);
    assert_eq!(
        json_error_as_format(&error).dart_to_string(),
        "FormatException: Invalid JSON number"
    );
}

#[test]
fn base64_errors_map_to_exact_dart_decoder_messages() {
    // convert/base64.dart:683-761: the decoder's four message forms.
    let invalid_char = convert_error_as_format(&opurom_core::ConvertError::InvalidBase64(0));
    assert_eq!(
        invalid_char.dart_to_string(),
        "FormatException: Invalid character"
    );
    let missing_padding = convert_error_as_format(&opurom_core::ConvertError::InvalidPadding);
    assert_eq!(
        missing_padding.dart_to_string(),
        "FormatException: Missing padding character"
    );
    let bad_encoding = convert_error_as_format(&opurom_core::ConvertError::InvalidEncoding);
    assert_eq!(
        bad_encoding.dart_to_string(),
        "FormatException: Invalid encoding before padding"
    );
    let bad_utf8 = convert_error_as_format(&opurom_core::ConvertError::InvalidUtf8(0));
    assert_eq!(
        bad_utf8.dart_to_string(),
        "FormatException: Malformed UTF-8 data"
    );
}

#[test]
fn format_exception_renders_caret_with_string_source() {
    // core/exceptions.dart:94-127,159-161: a string source plus an offset
    // renders a caret at character offset+1 (1-based).
    let error = DartError::FormatException {
        message: "Invalid character".to_owned(),
        source: Some("Zg*=".to_owned()),
        offset: Some(2),
    };
    assert_eq!(
        error.dart_to_string(),
        "FormatException: Invalid character (at character 3)\nZg*=\n  ^\n"
    );
}

#[test]
fn format_exception_truncates_long_sources() {
    // core/exceptions.dart:101-105: sources longer than 78 chars become
    // the first 75 chars plus "...".
    let source = "x".repeat(100);
    let error = DartError::FormatException {
        message: "bad".to_owned(),
        source: Some(source),
        offset: None,
    };
    let rendered = error.dart_to_string();
    assert!(rendered.contains(&"x".repeat(75)));
    assert!(rendered.ends_with("..."));
    assert!(!rendered.contains(&"x".repeat(76)));
}

#[test]
fn format_exception_ignores_out_of_range_offset() {
    // core/exceptions.dart:96-99: an offset outside [0, length] is dropped.
    let error = DartError::FormatException {
        message: "bad".to_owned(),
        source: Some("abc".to_owned()),
        offset: Some(99),
    };
    assert_eq!(error.dart_to_string(), "FormatException: bad\nabc");
}

#[test]
fn reduce_empty_error_is_distinct_from_format_error() {
    // core/iterable.dart:392-396: reduce throws IterableElementError on an
    // empty iterable, not a format error.
    let error = reduce::<i64, _>(&[], |a, b| a + b).unwrap_err();
    assert_eq!(error, NoElementError);
    assert_eq!(error.to_string(), "IterableElementError.noElement");
}

#[test]
fn json_decoded_collections_feed_dart_collection_helpers() {
    // convert/json.dart:157-160 returns lists/maps; core/iterable.dart:392,425
    // operators must work on the decoded tree without an interpreter.
    let value = json_decode("[10,2,5,1]").unwrap();
    let opurom_core::FutureValue::List(ref numbers) = value else {
        panic!("expected list");
    };
    let values: Vec<i64> = numbers
        .iter()
        .map(|value| match value {
            opurom_core::FutureValue::Int(value) => *value,
            other => panic!("expected int, got {other:?}"),
        })
        .collect();
    assert_eq!(reduce(&values, |a, b| a + b), Ok(18));
    let evens = collect_where(&values, |value| value % 2 == 0);
    assert_eq!(evens, vec![10, 2]);
    assert_eq!(json_encode(&value).unwrap(), "[10,2,5,1]");
}

#[test]
fn error_to_string_is_deterministic_across_instances() {
    let a = DartError::FormatException {
        message: "boom".to_owned(),
        source: Some("a\nb".to_owned()),
        offset: Some(3),
    };
    let b = a.clone();
    assert_eq!(a.dart_to_string(), b.dart_to_string());
    assert_eq!(a.dart_to_string().as_bytes(), b.dart_to_string().as_bytes());
}

#[test]
fn format_exception_reports_lf_line_and_column() {
    // core/exceptions.dart:107-127: line and character are 1-based; an LF
    // starts a new line.
    let error = DartError::FormatException {
        message: "bad".to_owned(),
        source: Some("ab\ncd".to_owned()),
        offset: Some(3),
    };
    assert_eq!(
        error.dart_to_string(),
        "FormatException: bad (at line 2, character 1)\ncd\n^\n"
    );
}

#[test]
fn format_exception_treats_crlf_as_one_line_break() {
    // core/exceptions.dart:112-121: LF after CR does not increment twice.
    let error = DartError::FormatException {
        message: "bad".to_owned(),
        source: Some("ab\r\ncd".to_owned()),
        offset: Some(4),
    };
    assert_eq!(
        error.dart_to_string(),
        "FormatException: bad (at line 2, character 1)\ncd\n^\n"
    );
}

#[test]
fn format_exception_limits_long_line_to_context_window() {
    // core/exceptions.dart:142-161: with the offset far from both ends,
    // only 36 characters on each side are shown with ellipses.
    let source = format!("{}X{}", "a".repeat(100), "b".repeat(100));
    let error = DartError::FormatException {
        message: "bad".to_owned(),
        source: Some(source),
        offset: Some(100),
    };
    let rendered = error.dart_to_string();
    assert!(rendered.contains("..."));
    assert!(rendered.contains('X'));
    assert!(rendered.ends_with("^\n"));
}
