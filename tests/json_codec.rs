//! Regression tests for the core JSON codec, pinned to Dart 3.14.0
//! (`flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb).

use opurom_core::{json_decode, json_encode, json_encode_pretty, FutureValue, JsonError};

fn text(value: &str) -> FutureValue {
    FutureValue::Str(value.to_owned())
}

#[test]
fn compact_encoding_matches_dart_example() {
    // convert/json.dart:83-88: key order is preserved from the source map.
    let value = FutureValue::Map(vec![
        ("text".to_owned(), text("foo")),
        ("value".to_owned(), FutureValue::Int(2)),
        ("status".to_owned(), FutureValue::Bool(false)),
        ("extra".to_owned(), FutureValue::Null),
    ]);
    assert_eq!(
        json_encode(&value).unwrap(),
        r#"{"text":"foo","value":2,"status":false,"extra":null}"#
    );
}

#[test]
fn pretty_encoding_matches_dart_example() {
    // convert/json.dart:264-275: two-space indent, one entry per line.
    let value = FutureValue::Map(vec![
        ("text".to_owned(), text("foo")),
        ("value".to_owned(), text("2")),
    ]);
    assert_eq!(
        json_encode_pretty(&value, "  ").unwrap(),
        "{\n  \"text\": \"foo\",\n  \"value\": \"2\"\n}"
    );
}

#[test]
fn decode_returns_numbers_strings_bools_and_null() {
    // convert/json.dart:63-65,157-160: Dart decodes num/string/bool/null.
    assert_eq!(
        json_decode(r#"["foo",{"bar":499},true,null,1.5]"#).unwrap(),
        FutureValue::List(vec![
            text("foo"),
            FutureValue::Map(vec![("bar".to_owned(), FutureValue::Int(499))]),
            FutureValue::Bool(true),
            FutureValue::Null,
            FutureValue::Float(1.5),
        ])
    );
}

#[test]
fn integer_and_double_tokens_are_distinguished() {
    assert_eq!(json_decode("42").unwrap(), FutureValue::Int(42));
    assert_eq!(json_decode("4e2").unwrap(), FutureValue::Float(400.0));
    assert_eq!(json_decode("-3").unwrap(), FutureValue::Int(-3));
}

#[test]
fn nested_round_trip_is_byte_stable() {
    let value = FutureValue::Map(vec![
        (
            "list".to_owned(),
            FutureValue::List(vec![FutureValue::Int(1), FutureValue::Int(2)]),
        ),
        (
            "nested".to_owned(),
            FutureValue::Map(vec![("ok".to_owned(), FutureValue::Bool(true))]),
        ),
    ]);
    let encoded = json_encode(&value).unwrap();
    assert_eq!(json_decode(&encoded).unwrap(), value);
    assert_eq!(
        json_encode(&json_decode(&encoded).unwrap()).unwrap(),
        encoded
    );
}

#[test]
fn strings_escape_special_characters() {
    let value = text("a\"b\\c\n\t");
    let encoded = json_encode(&value).unwrap();
    assert_eq!(encoded, r#""a\"b\\c\n\t""#);
    assert_eq!(json_decode(&encoded).unwrap(), value);
}

#[test]
fn invalid_json_is_typed() {
    assert!(matches!(json_decode("{"), Err(JsonError::InvalidJson(_))));
    assert!(matches!(
        json_decode("not json"),
        Err(JsonError::InvalidJson(_))
    ));
}

#[test]
fn non_finite_floats_are_rejected() {
    assert_eq!(
        json_encode(&FutureValue::Float(f64::NAN)),
        Err(JsonError::NonFiniteFloat)
    );
    assert_eq!(
        json_encode(&FutureValue::Float(f64::INFINITY)),
        Err(JsonError::NonFiniteFloat)
    );
}

#[test]
fn empty_containers_encode_compactly() {
    assert_eq!(json_encode(&FutureValue::List(vec![])).unwrap(), "[]");
    assert_eq!(json_encode(&FutureValue::Map(vec![])).unwrap(), "{}");
    assert_eq!(
        json_encode_pretty(&FutureValue::List(vec![]), "  ").unwrap(),
        "[]"
    );
}
