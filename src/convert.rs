//! Deterministic conversion utilities ported from Dart 3.14.0 (`dart:convert`,
//! `dart:core`).
//!
//! Base checkout: `flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb` (`3.50.0`), Dart SDK `3.14.0`
//! revision `fa8c1270c374c1346931583a17c3b7f278d3822d`. Source paths and
//! line numbers are relative to `bin/cache/dart-sdk/lib/`.
//!
//! Ported rules:
//! - the standard Base64 alphabet and the URL-safe alphabet
//!   (`convert/base64.dart:281-286`); padding is required and generated so
//!   the output length is a multiple of four (`convert/base64.dart:62-66`);
//! - decoding accepts either alphabet and requires padding
//!   (`convert/base64.dart:9-11,23-27`);
//! - UTF-8 encodes a string to code units and decodes back; malformed input
//!   is an error unless replacement is explicitly requested
//!   (`convert/utf.dart:24-43`);
//! - `Uri.encodeComponent` percent-encodes bytes with the unreserved mask,
//!   leaving `A-Z a-z 0-9 -_.!~*'()` unescaped (`core/uri.dart:1240-1242`,
//!   `core/uri.dart:_unreserved2396Mask`);
//! - `Uri.encodeQueryComponent` uses the HTML form rules: space becomes `+`
//!   and only `A-Z a-z 0-9 -._~` stay literal
//!   (`core/uri.dart:1244-1257`).
//!
//! All functions are pure and byte-deterministic on every host.

/// Convert errors from malformed input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertError {
    /// Invalid UTF-8 sequence at this byte index.
    InvalidUtf8(usize),
    /// Invalid base64 character at this byte index.
    InvalidBase64(usize),
    /// Invalid base64 padding.
    InvalidPadding,
    /// Non-zero discarded bits before padding
    /// (`convert/base64.dart:741,747`).
    InvalidEncoding,
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUtf8(index) => write!(f, "invalid UTF-8 byte at {index}"),
            Self::InvalidBase64(index) => write!(f, "invalid base64 byte at {index}"),
            Self::InvalidPadding => f.write_str("invalid base64 padding"),
            Self::InvalidEncoding => f.write_str("invalid base64 encoding"),
        }
    }
}

impl std::error::Error for ConvertError {}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Whether the URL-safe alphabet is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base64Variant {
    /// `A-Z a-z 0-9 + /` (`convert/base64.dart:281-282`).
    Standard,
    /// `A-Z a-z 0-9 - _` (`convert/base64.dart:285-286`).
    UrlSafe,
}

/// Encodes bytes into base64 with padding (`convert/base64.dart:41,62-66`).
pub fn base64_encode(bytes: &[u8], variant: Base64Variant) -> String {
    let alphabet = match variant {
        Base64Variant::Standard => BASE64_ALPHABET,
        Base64Variant::UrlSafe => BASE64_URL_ALPHABET,
    };
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let bits = (b0 << 16) | (b1 << 8) | b2;
        out.push(alphabet[((bits >> 18) & 0x3f) as usize] as char);
        out.push(alphabet[((bits >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            alphabet[((bits >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            alphabet[(bits & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Decodes base64, accepting both alphabets (`convert/base64.dart:9-11`).
/// Invalid characters and missing padding are errors
/// (`convert/base64.dart:62-66`).
pub fn base64_decode(source: &str) -> Result<Vec<u8>, ConvertError> {
    let bytes = source.as_bytes();
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if bytes.len() % 4 != 0 {
        return Err(ConvertError::InvalidPadding);
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (block, chunk) in bytes.chunks(4).enumerate() {
        let mut sextets = [0u8; 4];
        let mut padding = 0usize;
        for (index, byte) in chunk.iter().copied().enumerate() {
            if byte == b'=' {
                if index < 2 {
                    return Err(ConvertError::InvalidPadding);
                }
                padding += 1;
                sextets[index] = 0;
                continue;
            }
            let value =
                decode_sextet(byte).ok_or(ConvertError::InvalidBase64(block * 4 + index))?;
            sextets[index] = value;
        }
        if padding > 2 {
            return Err(ConvertError::InvalidPadding);
        }
        // base64.dart:737-758 accumulates only the *non-padding* sextets.
        // The low bits of that accumulator must be zero before padding.
        let data_count = 4 - padding;
        let mut bits = 0u32;
        for &sextet in sextets.iter().take(data_count) {
            bits = ((bits << 6) | sextet as u32) & 0x00ff_ffff;
        }
        if (padding == 1 && bits & 0x03 != 0) || (padding == 2 && bits & 0x0f != 0) {
            return Err(ConvertError::InvalidEncoding);
        }
        // Reassemble the full 24-bit block for byte extraction.
        let bits = ((sextets[0] as u32) << 18)
            | ((sextets[1] as u32) << 12)
            | ((sextets[2] as u32) << 6)
            | sextets[3] as u32;
        out.push((bits >> 16) as u8);
        if padding < 2 {
            out.push((bits >> 8) as u8);
        }
        if padding < 1 {
            out.push(bits as u8);
        }
    }
    Ok(out)
}

fn decode_sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

/// Encodes a string to UTF-8 bytes (`convert/utf.dart:24-43`).
pub fn utf8_encode(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

/// Decodes UTF-8 bytes. Malformed sequences are errors unless replacement
/// is requested (`convert/utf.dart:32-40`).
pub fn utf8_decode(bytes: &[u8], allow_malformed: bool) -> Result<String, ConvertError> {
    let mut out = String::new();
    let mut index = 0;
    while index < bytes.len() {
        let first = bytes[index];
        let (code_point, width) = if first < 0x80 {
            (first as u32, 1)
        } else if first & 0xe0 == 0xc0 {
            (first as u32 & 0x1f, 2)
        } else if first & 0xf0 == 0xe0 {
            (first as u32 & 0x0f, 3)
        } else if first & 0xf8 == 0xf0 {
            (first as u32 & 0x07, 4)
        } else {
            if allow_malformed {
                out.push('\u{fffd}');
                index += 1;
                continue;
            }
            return Err(ConvertError::InvalidUtf8(index));
        };
        if index + width > bytes.len() {
            if allow_malformed {
                out.push('\u{fffd}');
                break;
            }
            return Err(ConvertError::InvalidUtf8(index));
        }
        let mut value = code_point;
        let mut malformed = false;
        for offset in 1..width {
            let continuation = bytes[index + offset];
            if continuation & 0xc0 != 0x80 {
                if allow_malformed {
                    out.push('\u{fffd}');
                    malformed = true;
                    break;
                }
                return Err(ConvertError::InvalidUtf8(index + offset));
            }
            value = (value << 6) | (continuation & 0x3f) as u32;
        }
        if !malformed {
            if let Some(character) = char::from_u32(value) {
                out.push(character);
            } else if allow_malformed {
                out.push('\u{fffd}');
            } else {
                return Err(ConvertError::InvalidUtf8(index));
            }
        }
        index += width;
    }
    Ok(out)
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
        )
}

fn percent_encode(bytes: &[u8], keep: fn(u8) -> bool, space_as_plus: bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::new();
    for &byte in bytes {
        if space_as_plus && byte == b' ' {
            out.push('+');
        } else if keep(byte) {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out
}

/// `Uri.encodeComponent` (`core/uri.dart:1240-1242`).
pub fn uri_encode_component(text: &str) -> String {
    percent_encode(text.as_bytes(), is_unreserved, false)
}

/// `Uri.encodeQueryComponent` (`core/uri.dart:1244-1257`): spaces become `+`
/// and only `A-Z a-z 0-9 -._~` stay literal.
pub fn uri_encode_query_component(text: &str) -> String {
    percent_encode(
        text.as_bytes(),
        |byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'),
        true,
    )
}

/// Clamps `value` into `[min, max]` (`core/num.dart:min/max`).
pub fn clamp_i64(value: i64, min: i64, max: i64) -> i64 {
    value.clamp(min, max)
}
