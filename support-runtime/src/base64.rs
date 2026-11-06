//! The base64 text form of binary content carried inside a structured body:
//! `contentEncoding: base64`.
//!
//! JSON Schema 2020-12 — the dialect OpenAPI 3.1 and 3.2 use — says a string with
//! `contentEncoding: base64` holds binary content encoded with RFC 4648's base64: the standard
//! alphabet (`A`–`Z`, `a`–`z`, `0`–`9`, `+`, `/`), padded with `=`. `bytes::Bytes`' own `serde`
//! implementation writes a byte sequence, which JSON spells as an array of numbers, so a generated
//! model never relies on it for such a value. Each schema with the keyword is emitted as a newtype
//! over `bytes::Bytes` whose `Serialize` and `Deserialize` are [`serialize_base64`] and
//! [`deserialize_base64`], so the value is that text wherever it sits: a field, an array item, a map
//! value, a union variant or a tuple position, in JSON, XML or a form body alike.
//!
//! Encoding always writes the padded standard alphabet. Decoding takes the same alphabet with or
//! without its trailing padding, and refuses anything else: another alphabet's characters
//! (base64url's `-` and `_`), whitespace or line breaks, padding before the end, or a length no
//! encoding produces.

use std::fmt;

use bytes::Bytes;
use serde::de::{Deserializer, Unexpected, Visitor};
use serde::ser::Serializer;

/// The RFC 4648 standard alphabet, indexed by sextet value.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Writes `bytes` as its padded standard base64 text.
pub fn serialize_base64<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&encode(bytes))
}

/// Reads a padded or unpadded standard base64 string into the bytes it encodes.
pub fn deserialize_base64<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_str(Base64Visitor)
}

/// Accepts a string and decodes it; any other value is refused as not base64 text.
struct Base64Visitor;

impl Visitor<'_> for Base64Visitor {
    type Value = Bytes;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a base64 string (RFC 4648, standard alphabet)")
    }

    fn visit_str<E>(self, value: &str) -> Result<Bytes, E>
    where
        E: serde::de::Error,
    {
        decode(value)
            .map(Bytes::from)
            .ok_or_else(|| E::invalid_value(Unexpected::Str(value), &self))
    }
}

/// `bytes` as padded standard base64.
fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk.iter().enumerate().fold(0u32, |group, (index, byte)| {
            group | (u32::from(*byte) << (16 - 8 * index))
        });
        // A chunk of `n` bytes fills `n + 1` sextets; the rest of the quantum is padding.
        for sextet in 0..4 {
            if sextet <= chunk.len() {
                let value = (group >> (18 - 6 * sextet)) & 0x3f;
                out.push(char::from(ALPHABET[value as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The sextet value of one standard-alphabet character, or `None` for any other byte.
fn sextet(character: u8) -> Option<u32> {
    let value = match character {
        b'A'..=b'Z' => character - b'A',
        b'a'..=b'z' => character - b'a' + 26,
        b'0'..=b'9' => character - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

/// The bytes `text` encodes, or `None` when it is not standard base64. Trailing padding is
/// optional, but where present it must complete the final quantum exactly.
fn decode(text: &str) -> Option<Vec<u8>> {
    let text = text.as_bytes();
    let unpadded = text
        .iter()
        .rposition(|character| *character != b'=')
        .map_or(0, |last| last + 1);
    let padding = text.len() - unpadded;
    let data = &text[..unpadded];
    // A final quantum of one character carries no whole byte, and padding is either absent or
    // exactly what completes the last quantum to four characters.
    if data.len() % 4 == 1 || padding > 2 || (padding > 0 && text.len() % 4 != 0) {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 2);
    for chunk in data.chunks(4) {
        let mut group = 0u32;
        for (index, character) in chunk.iter().enumerate() {
            group |= sextet(*character)? << (18 - 6 * index);
        }
        // `n` characters carry `n - 1` whole bytes.
        for index in 0..chunk.len() - 1 {
            out.push((group >> (16 - 8 * index)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use proptest::prelude::*;
    use serde::{Deserialize, Serialize};

    /// A field carrying the codec, as a generated newtype does.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Payload(
        #[serde(
            serialize_with = "super::serialize_base64",
            deserialize_with = "super::deserialize_base64"
        )]
        Bytes,
    );

    /// RFC 4648 §10's test vectors, padded.
    const VECTORS: &[(&str, &str)] = &[
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ];

    #[test]
    fn encoding_writes_the_rfc_4648_vectors() {
        for (plain, encoded) in VECTORS {
            assert_eq!(super::encode(plain.as_bytes()), *encoded, "{plain:?}");
        }
    }

    #[test]
    fn decoding_reads_the_rfc_4648_vectors_padded_and_unpadded() {
        for (plain, encoded) in VECTORS {
            assert_eq!(
                super::decode(encoded).as_deref(),
                Some(plain.as_bytes()),
                "{encoded:?}"
            );
            let unpadded = encoded.trim_end_matches('=');
            assert_eq!(
                super::decode(unpadded).as_deref(),
                Some(plain.as_bytes()),
                "{unpadded:?}"
            );
        }
    }

    #[test]
    fn the_whole_alphabet_round_trips() {
        let bytes: Vec<u8> = (0..=255).collect();
        let encoded = super::encode(&bytes);
        assert!(encoded.contains('+') && encoded.contains('/'));
        assert_eq!(super::decode(&encoded), Some(bytes));
    }

    #[test]
    fn decoding_refuses_what_no_standard_encoding_writes() {
        for text in [
            "Z",          // one character of a quantum carries no byte
            "Zm9vY",      // the same after a whole quantum
            "Zg===",      // padding past the quantum
            "Zg=",        // padding that does not complete the quantum
            "=Zg=",       // padding before the data
            "Zg==Zg==",   // padding inside the data
            "Zm9v Yg==",  // whitespace
            "Zm9v\nYg==", // a line break
            "-_8=",       // the base64url alphabet
            "Zm9v!",      // a character of no alphabet
            "====",       // padding alone
        ] {
            assert_eq!(super::decode(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_json_member_is_a_base64_string() {
        let payload = Payload(Bytes::from_static(b"d4:infoe"));
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json, serde_json::json!("ZDQ6aW5mb2U="));
        assert_eq!(serde_json::from_value::<Payload>(json).unwrap(), payload);
    }

    #[test]
    fn a_json_value_that_is_not_base64_text_is_refused() {
        for json in [
            serde_json::json!("not base64!"),
            serde_json::json!([100, 52]),
            serde_json::json!(null),
        ] {
            let error = serde_json::from_value::<Payload>(json.clone()).unwrap_err();
            assert!(
                error.to_string().contains("base64 string"),
                "{json}: {error}"
            );
        }
    }

    proptest! {
        #[test]
        fn every_byte_string_round_trips(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let encoded = super::encode(&bytes);
            prop_assert_eq!(encoded.len() % 4, 0);
            prop_assert_eq!(super::decode(&encoded), Some(bytes.clone()));
            prop_assert_eq!(super::decode(encoded.trim_end_matches('=')), Some(bytes));
        }
    }
}
