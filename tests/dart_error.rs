//! Regression tests for the Dart-shaped error taxonomy, pinned to Dart 3.14.0
//! (`flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb).

use opurom_core::{
    check_not_null, check_valid_index, check_value_in_interval, DartError, DartStackTrace,
};

#[test]
fn assertion_string_matches_dart() {
    // core/errors.dart:135-140.
    assert_eq!(
        DartError::Assertion { message: None }.dart_to_string(),
        "Assertion failed"
    );
    assert_eq!(
        DartError::Assertion {
            message: Some("x > 0".to_owned())
        }
        .dart_to_string(),
        "Assertion failed: x > 0"
    );
}

#[test]
fn argument_error_composes_name_message_and_value() {
    // core/errors.dart:223-238: Invalid argument (name): message: value.
    let error = DartError::Argument {
        name: Some("foo".to_owned()),
        message: Some("expected positive".to_owned()),
        invalid_value: Some("-1".to_owned()),
    };
    assert_eq!(
        error.dart_to_string(),
        "Invalid argument (foo): expected positive: -1"
    );
    let error = DartError::Argument {
        name: None,
        message: None,
        invalid_value: None,
    };
    assert_eq!(error.dart_to_string(), "Invalid argument(s)");
}

#[test]
fn not_null_error_uses_dart_message() {
    // core/errors.dart:208-221.
    assert_eq!(
        check_not_null::<i64>(None, Some("value")).unwrap_err(),
        DartError::Argument {
            name: Some("value".to_owned()),
            message: Some("Must not be null".to_owned()),
            invalid_value: None,
        }
    );
    assert_eq!(check_not_null(Some(7), Some("value")), Ok(7));
}

#[test]
fn range_error_uses_interval_bounds() {
    // core/errors.dart:280-288,315-327.
    assert_eq!(check_value_in_interval(5, 0, 10), Ok(5));
    assert_eq!(
        check_value_in_interval(11, 0, 10)
            .unwrap_err()
            .dart_to_string(),
        "RangeError: Invalid value[0..10]: 11"
    );
}

#[test]
fn index_error_reports_length() {
    // core/errors.dart:445-475,329-350.
    assert_eq!(check_valid_index(2, 5), Ok(2));
    assert_eq!(
        check_valid_index(5, 5).unwrap_err().dart_to_string(),
        "RangeError (index): Index out of range: 5, length 5"
    );
    assert_eq!(
        check_valid_index(-1, 5).unwrap_err().dart_to_string(),
        "RangeError (index): Index out of range: -1, length 5"
    );
}

#[test]
fn state_and_support_errors_match_dart_prefixes() {
    // core/errors.dart:580-603,619-624,631-643.
    assert_eq!(
        DartError::State("No element".to_owned()).dart_to_string(),
        "Bad state: No element"
    );
    assert_eq!(
        DartError::Unsupported("closed".to_owned()).dart_to_string(),
        "Unsupported operation: closed"
    );
    assert_eq!(
        DartError::Unimplemented(None).dart_to_string(),
        "UnimplementedError"
    );
    assert_eq!(
        DartError::ConcurrentModification(None).dart_to_string(),
        "Concurrent modification during iteration."
    );
}

#[test]
fn platform_errors_match_dart() {
    assert_eq!(DartError::OutOfMemory.dart_to_string(), "Out of Memory");
    assert_eq!(DartError::StackOverflow.dart_to_string(), "Stack Overflow");
    assert_eq!(DartError::Type.dart_to_string(), "TypeError");
}

#[test]
fn stack_trace_is_immutable_string() {
    // core/stacktrace.dart:19,33,53-56.
    assert_eq!(DartStackTrace::empty().as_str(), "");
    let trace = DartStackTrace::from_string("stack line");
    assert_eq!(trace.to_string(), "stack line");
    assert_eq!(trace.as_str(), "stack line");
}
