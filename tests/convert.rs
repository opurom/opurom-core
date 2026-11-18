//! Regression tests for the Dart 3.14 conversion utilities, pinned to
//! `flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb
//! (Dart SDK 3.14.0).

use opurom_core::{
    base64_decode, base64_encode, clamp_i64, convert::utf8_decode, convert::utf8_encode,
    uri_encode_component, uri_encode_query_component, Base64Variant, ConvertError,
};

#[test]
fn base64_standard_matches_dart_example() {
    // convert/base64.dart:15-17: the SDK's own byte array. RFC 4648 encodes
    // these 13 bytes with two trailing '='; the doc string in the SDK shows
    // a single '=' and is inconsistent with its own array, so the byte-level
    // result from the alphabet (base64.dart:281-282) is authoritative here.
    let bytes = [
        0x62, 0x6c, 0xc3, 0xa5, 0x62, 0xc3, 0xa6, 0x72, 0x67, 0x72, 0xc3, 0xb8, 0x64,
    ];
    assert_eq!(
        base64_encode(&bytes, Base64Variant::Standard),
        "YmzDpWLDpnJncsO4ZA=="
    );
    assert_eq!(base64_decode("YmzDpWLDpnJncsO4ZA=="), Ok(bytes.to_vec()));
}

#[test]
fn base64_uses_padding() {
    // convert/base64.dart:62-66: output is always a multiple of four.
    assert_eq!(base64_encode(b"f", Base64Variant::Standard), "Zg==");
    assert_eq!(base64_encode(b"fo", Base64Variant::Standard), "Zm8=");
    assert_eq!(base64_encode(b"foo", Base64Variant::Standard), "Zm9v");
}

#[test]
fn base64_url_safe_alphabet() {
    // convert/base64.dart:23-35,285-286: the URL alphabet uses - and _.
    let bytes = [0xfb, 0xff, 0xbf];
    assert_eq!(base64_encode(&bytes, Base64Variant::UrlSafe), "-_-_");
    // The decoder accepts both alphabets (convert/base64.dart:25-27).
    assert_eq!(base64_decode("-_-_"), Ok(bytes.to_vec()));
    assert_eq!(base64_decode("+/+/"), Ok(bytes.to_vec()));
}

#[test]
fn base64_rejects_invalid_input() {
    // convert/base64.dart:9-11: invalid characters and missing padding are
    // rejected.
    assert_eq!(base64_decode("Zg"), Err(ConvertError::InvalidPadding));
    assert!(matches!(
        base64_decode("Zg*="),
        Err(ConvertError::InvalidBase64(2))
    ));
    assert_eq!(base64_decode("=Zg="), Err(ConvertError::InvalidPadding));
}

#[test]
fn utf8_round_trip() {
    // convert/utf.dart:24-43: encode to code units and decode back.
    let text = "hello 世界 🌍";
    let bytes = utf8_encode(text);
    assert_eq!(utf8_decode(&bytes, false), Ok(text.to_owned()));
}

#[test]
fn utf8_rejects_malformed_unless_replacement_allowed() {
    // convert/utf.dart:32-40: malformed input throws by default; with
    // allowMalformed it is replaced by U+FFFD.
    assert_eq!(
        utf8_decode(&[0xff], false),
        Err(ConvertError::InvalidUtf8(0))
    );
    assert_eq!(utf8_decode(&[0xff], true), Ok("\u{fffd}".to_owned()));
    assert_eq!(utf8_decode(&[0xc3, 0x28], true), Ok("\u{fffd}".to_owned()));
}

#[test]
fn uri_component_encoding_keeps_unreserved_characters() {
    // core/uri.dart:1240-1242 with the unreserved 2396 mask.
    assert_eq!(
        uri_encode_component(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()"
        ),
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()"
    );
    assert_eq!(uri_encode_component("a b/c?d"), "a%20b%2Fc%3Fd");
    assert_eq!(uri_encode_component("世界"), "%E4%B8%96%E7%95%8C");
}

#[test]
fn query_component_encoding_uses_plus_for_space() {
    // core/uri.dart:1244-1257: spaces become '+' and only A-Z a-z 0-9 -._~
    // stay literal.
    assert_eq!(
        uri_encode_query_component("a b!~*'()"),
        "a+b%21~%2A%27%28%29"
    );
    assert_eq!(uri_encode_query_component("x/y"), "x%2Fy");
}

#[test]
fn clamp_matches_bounds() {
    assert_eq!(clamp_i64(5, 0, 10), 5);
    assert_eq!(clamp_i64(-1, 0, 10), 0);
    assert_eq!(clamp_i64(11, 0, 10), 10);
}

#[test]
fn base64_rejects_non_canonical_padding_bits() {
    // convert/base64.dart:737-758: low bits before '=' must be zero.
    // "Zh==" and "Zi==" have non-zero discarded bits (h=33, i=34), while
    // "Zg==" is canonical for 'f'. "Zm9=" has a non-zero final sextet.
    assert_eq!(base64_decode("Zh=="), Err(ConvertError::InvalidEncoding));
    assert_eq!(base64_decode("Zi=="), Err(ConvertError::InvalidEncoding));
    assert_eq!(base64_decode("Zm9="), Err(ConvertError::InvalidEncoding));
    assert_eq!(base64_decode("Zg=="), Ok(vec![b'f']));
    assert_eq!(base64_decode("Zm8="), Ok(b"fo".to_vec()));
}

#[test]
fn base64_empty_and_round_trip_all_byte_values() {
    assert_eq!(base64_decode(""), Ok(Vec::new()));
    assert_eq!(base64_encode(&[], Base64Variant::Standard), "");
    let bytes: Vec<u8> = (0..=255).collect();
    for variant in [Base64Variant::Standard, Base64Variant::UrlSafe] {
        let encoded = base64_encode(&bytes, variant);
        assert_eq!(encoded.len(), 344);
        assert_eq!(base64_decode(&encoded), Ok(bytes.clone()));
    }
}

#[test]
fn utf8_boundary_code_points_round_trip() {
    // convert/utf.dart:24-43: all scalar values must round-trip.
    for code_point in [0x7f, 0x80, 0x7ff, 0x800, 0xffff, 0x10000, 0x10ffff] {
        if let Some(character) = char::from_u32(code_point) {
            let text = character.to_string();
            assert_eq!(utf8_decode(&utf8_encode(&text), false), Ok(text));
        }
    }
}

#[test]
fn uri_encoding_handles_empty_and_binary_text() {
    assert_eq!(uri_encode_component(""), "");
    assert_eq!(uri_encode_query_component(""), "");
    assert_eq!(uri_encode_component("\u{0000}"), "%00");
    assert_eq!(
        uri_encode_query_component("世界 / 你好"),
        "%E4%B8%96%E7%95%8C+%2F+%E4%BD%A0%E5%A5%BD"
    );
}

#[test]
fn clamp_handles_full_i64_range() {
    assert_eq!(clamp_i64(i64::MIN, -5, 5), -5);
    assert_eq!(clamp_i64(i64::MAX, -5, 5), 5);
    assert_eq!(clamp_i64(0, i64::MIN, i64::MAX), 0);
}
