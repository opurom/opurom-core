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

#[test]
fn i64_boundary_integers_decode_exactly() {
    // core/int.dart:7-10: Dart's VM int is signed 64-bit two's complement.
    assert_eq!(
        json_decode("9223372036854775807").unwrap(),
        FutureValue::Int(i64::MAX)
    );
    assert_eq!(
        json_decode("-9223372036854775808").unwrap(),
        FutureValue::Int(i64::MIN)
    );
    assert_eq!(
        json_encode(&FutureValue::Int(i64::MAX)).unwrap(),
        "9223372036854775807"
    );
}

#[test]
fn integer_above_i64_max_never_wraps_negative() {
    // core/int.dart:7-16: no silent two's-complement wrap; a value outside
    // the signed 64-bit range stays a double (as with Dart's JS backend).
    let value = json_decode("9223372036854775808").unwrap();
    assert!(
        matches!(value, FutureValue::Float(number) if number > 9.0e18),
        "{value:?}"
    );
    let value = json_decode("18446744073709551615").unwrap();
    assert!(
        matches!(value, FutureValue::Float(number) if number > 0.0),
        "{value:?}"
    );
}

#[test]
fn double_precision_boundaries_are_deterministic() {
    // core/double.dart:7-12: IEEE 754 binary64. Values that are exactly
    // representable must round-trip byte-identically.
    for token in ["1.5", "0.1", "1e308", "5e-324", "2.2250738585072014e-308"] {
        let first = json_decode(token).unwrap();
        let encoded = json_encode(&first).unwrap();
        let second = json_decode(&encoded).unwrap();
        assert_eq!(first, second, "token {token} encoded as {encoded}");
    }
}

#[test]
fn overflowing_exponent_is_a_typed_json_error() {
    // A literal that overflows binary64 has no finite Dart double value
    // (core/double.dart:7-12), so it is rejected rather than becoming inf.
    assert!(matches!(
        json_decode("1e400"),
        Err(JsonError::InvalidJson(_))
    ));
}

#[test]
fn surrogate_pair_escapes_decode_to_single_code_point() {
    // convert/json.dart:683-686,1143-1152: a lead+trail surrogate pair is
    // one Unicode scalar in UTF-8 output.
    let value = json_decode(r#""🌍""#).unwrap();
    assert_eq!(value, text("🌍"));
    assert_eq!(json_encode(&value).unwrap(), r#""🌍""#);
}

#[test]
fn escaped_control_characters_and_solidus_round_trip() {
    // convert/json.dart:714-745: control chars are escaped as \u00XX; the
    // solidus may be escaped but does not have to be.
    let value = json_decode(r#""a\/b\u0000c""#).unwrap();
    assert_eq!(value, text("a/b\0c"));
    let encoded = json_encode(&value).unwrap();
    assert_eq!(json_decode(&encoded).unwrap(), value);
}

#[test]
fn utf8_bom_is_a_real_character_not_silently_dropped() {
    // convert/utf.dart:8-11: U+FEFF is a Unicode character; the codec does
    // not strip it implicitly.
    let bytes = [0xEF, 0xBB, 0xBF, b'A'];
    assert_eq!(
        opurom_core::utf8_decode(&bytes, false).unwrap(),
        "\u{feff}A"
    );
    assert_eq!(opurom_core::utf8_encode("\u{feff}A"), bytes);
}

#[test]
fn truncated_bom_is_malformed_utf8() {
    // convert/utf.dart:571: an unterminated sequence is a FormatException
    // unless malformed input is explicitly allowed.
    let partial = [0xEF, 0xBB];
    assert!(opurom_core::utf8_decode(&partial, false).is_err());
    assert!(opurom_core::utf8_decode(&partial, true)
        .unwrap()
        .contains('\u{fffd}'));
}

#[test]
fn overlong_and_invalid_utf8_sequences_are_rejected() {
    // convert/utf.dart:32-40: malformed octet sequences throw by default.
    assert!(opurom_core::utf8_decode(&[0xC0, 0xAF], false).is_err()); // overlong '/'
    assert!(opurom_core::utf8_decode(&[0xED, 0xA0, 0x80], false).is_err()); // surrogate
    assert!(opurom_core::utf8_decode(&[0xF4, 0x90, 0x80, 0x80], false).is_err()); // > U+10FFFF
    assert_eq!(
        opurom_core::utf8_decode(&[0xF0, 0x9F, 0x8C, 0x8D], false).unwrap(),
        "🌍"
    );
}

#[test]
fn json_string_raw_newline_is_invalid() {
    // convert/json.dart:592: unescaped control characters are invalid JSON.
    assert!(matches!(
        json_decode("\"line\nbreak\""),
        Err(JsonError::InvalidJson(_))
    ));
}
