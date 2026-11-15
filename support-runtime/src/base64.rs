//! The base64 text forms of binary content carried inside a structured body:
//! `contentEncoding: base64` and `contentEncoding: base64url`.
//!
//! JSON Schema 2020-12 — the dialect OpenAPI 3.1 and 3.2 use — says a string with
//! `contentEncoding: base64` holds binary content encoded with RFC 4648's base64: the standard
//! alphabet (`A`–`Z`, `a`–`z`, `0`–`9`, `+`, `/`), padded with `=`. `base64url` is RFC 4648 §5's
//! URL- and filename-safe variant, the same encoding with `-` and `_` in place of `+` and `/`.
//! `bytes::Bytes`' own `serde` implementation writes a byte sequence, which JSON spells as an array
//! of numbers, so a generated model never relies on it for such a value. Each schema with the
//! keyword is emitted as a newtype over `bytes::Bytes` whose `Serialize` and `Deserialize` are
//! [`serialize_base64`] and [`deserialize_base64`], or [`serialize_base64url`] and
//! [`deserialize_base64url`], so the value is that text wherever it sits: a field, an array item, a
//! map value, a union variant or a tuple position, in JSON, XML or a form body alike.
//!
//! Encoding always writes the padded text of its alphabet, since RFC 4648 §3.2 asks for the padding
//! unless the referring specification says otherwise, and OpenAPI does not. Decoding takes the same
//! alphabet with or without its trailing padding, and refuses anything else: the other alphabet's
//! characters, whitespace or line breaks, padding before the end, a length no encoding produces, or
//! a final quantum whose bits past its last whole byte are not zero.

use std::fmt;

use bytes::Bytes;
use serde::de::{Deserializer, Unexpected, Visitor};
use serde::ser::Serializer;

/// One of the two RFC 4648 alphabets a `contentEncoding` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alphabet {
    /// `base64`, RFC 4648 §4.
    Standard,
    /// `base64url`, RFC 4648 §5.
    UrlSafe,
}

impl Alphabet {
    /// The alphabet's characters, indexed by sextet value.
    fn characters(self) -> &'static [u8; 64] {
        match self {
            Alphabet::Standard => {
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
            }
            Alphabet::UrlSafe => {
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
            }
        }
    }

    /// The sextet value of one character of this alphabet, or `None` for any other byte.
    fn sextet(self, character: u8) -> Option<u32> {
        let value = match (character, self) {
            (b'A'..=b'Z', _) => character - b'A',
            (b'a'..=b'z', _) => character - b'a' + 26,
            (b'0'..=b'9', _) => character - b'0' + 52,
            (b'+', Alphabet::Standard) | (b'-', Alphabet::UrlSafe) => 62,
            (b'/', Alphabet::Standard) | (b'_', Alphabet::UrlSafe) => 63,
            _ => return None,
        };
        Some(u32::from(value))
    }
}

/// Writes `bytes` as its padded standard base64 text.
pub fn serialize_base64<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&encode(bytes, Alphabet::Standard))
}

/// Reads a padded or unpadded standard base64 string into the bytes it encodes.
pub fn deserialize_base64<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_str(Base64Visitor(Alphabet::Standard))
}

/// Writes `bytes` as its padded base64url text.
pub fn serialize_base64url<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&encode(bytes, Alphabet::UrlSafe))
}

/// Reads a padded or unpadded base64url string into the bytes it encodes.
pub fn deserialize_base64url<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_str(Base64Visitor(Alphabet::UrlSafe))
}

/// Accepts a string and decodes it in its alphabet; any other value is refused as not that text.
struct Base64Visitor(Alphabet);

impl Visitor<'_> for Base64Visitor {
    type Value = Bytes;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            Alphabet::Standard => "a base64 string (RFC 4648, standard alphabet)",
            Alphabet::UrlSafe => "a base64url string (RFC 4648, URL-safe alphabet)",
        })
    }

    fn visit_str<E>(self, value: &str) -> Result<Bytes, E>
    where
        E: serde::de::Error,
    {
        decode(value, self.0)
            .map(Bytes::from)
            .ok_or_else(|| E::invalid_value(Unexpected::Str(value), &self))
    }
}

/// `bytes` as padded base64 text in `alphabet`.
fn encode(bytes: &[u8], alphabet: Alphabet) -> String {
    let characters = alphabet.characters();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk.iter().enumerate().fold(0u32, |group, (index, byte)| {
            group | (u32::from(*byte) << (16 - 8 * index))
        });
        // A chunk of `n` bytes fills `n + 1` sextets; the rest of the quantum is padding.
        for sextet in 0..4 {
            if sextet <= chunk.len() {
                let value = (group >> (18 - 6 * sextet)) & 0x3f;
                out.push(char::from(characters[value as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The bytes `text` encodes, or `None` when it is not base64 text in `alphabet`. Trailing padding
/// is optional, but where present it must complete the final quantum exactly.
fn decode(text: &str, alphabet: Alphabet) -> Option<Vec<u8>> {
    let text = text.as_bytes();
    let unpadded = text
        .iter()
        .rposition(|character| *character != b'=')
        .map_or(0, |last| last + 1);
    let padding = text.len() - unpadded;
    let data = &text[..unpadded];
    // A final quantum of one character carries no whole byte, and padding is either absent or
    // exactly what completes the last quantum to four characters.
    if data.len() % 4 == 1 || padding > 2 || (padding > 0 && !text.len().is_multiple_of(4)) {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 2);
    for chunk in data.chunks(4) {
        let mut group = 0u32;
        for (index, character) in chunk.iter().enumerate() {
            group |= alphabet.sextet(*character)? << (18 - 6 * index);
        }
        // `n` characters carry `n - 1` whole bytes; the bits a short final quantum holds past
        // them are zero in every encoding (RFC 4648 §3.5), so a set one is refused rather than
        // dropped, and each byte string has exactly one accepted spelling per padding form.
        let carried = 24 - 8 * (chunk.len() - 1);
        if group & ((1 << carried) - 1) != 0 {
            return None;
        }
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

    use super::Alphabet;

    /// A field carrying the codec, as a generated newtype does.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Payload(
        #[serde(
            serialize_with = "super::serialize_base64",
            deserialize_with = "super::deserialize_base64"
        )]
        Bytes,
    );

    /// A field carrying the base64url codec, as a generated newtype does.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct UrlPayload(
        #[serde(
            serialize_with = "super::serialize_base64url",
            deserialize_with = "super::deserialize_base64url"
        )]
        Bytes,
    );

    /// Standard base64 text of `bytes`.
    fn encode(bytes: &[u8]) -> String {
        super::encode(bytes, Alphabet::Standard)
    }

    /// The bytes standard base64 `text` encodes.
    fn decode(text: &str) -> Option<Vec<u8>> {
        super::decode(text, Alphabet::Standard)
    }

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
            assert_eq!(encode(plain.as_bytes()), *encoded, "{plain:?}");
        }
    }

    #[test]
    fn decoding_reads_the_rfc_4648_vectors_padded_and_unpadded() {
        for (plain, encoded) in VECTORS {
            assert_eq!(
                decode(encoded).as_deref(),
                Some(plain.as_bytes()),
                "{encoded:?}"
            );
            let unpadded = encoded.trim_end_matches('=');
            assert_eq!(
                decode(unpadded).as_deref(),
                Some(plain.as_bytes()),
                "{unpadded:?}"
            );
        }
    }

    #[test]
    fn the_whole_alphabet_round_trips() {
        let bytes: Vec<u8> = (0..=255).collect();
        let encoded = encode(&bytes);
        assert!(encoded.contains('+') && encoded.contains('/'));
        assert_eq!(decode(&encoded), Some(bytes));
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
            "Zh==",       // non-zero trailing bits after one byte
            "Zh",         // the same unpadded
            "Zm9=",       // non-zero trailing bits after two bytes
            "Zm9vZh==",   // the same after a whole quantum
        ] {
            assert_eq!(decode(text), None, "{text:?}");
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

    #[test]
    fn base64url_writes_the_url_safe_alphabet_padded() {
        // `0xfb 0xff` is `+/8=` in the standard alphabet, so it spells both characters that differ.
        assert_eq!(super::encode(&[0xfb, 0xff], Alphabet::UrlSafe), "-_8=");
        for (plain, encoded) in VECTORS {
            assert_eq!(
                super::encode(plain.as_bytes(), Alphabet::UrlSafe),
                *encoded,
                "{plain:?}"
            );
        }
        let bytes: Vec<u8> = (0..=255).collect();
        let encoded = super::encode(&bytes, Alphabet::UrlSafe);
        assert!(encoded.contains('-') && encoded.contains('_'));
        assert!(!encoded.contains('+') && !encoded.contains('/'));
        assert_eq!(super::decode(&encoded, Alphabet::UrlSafe), Some(bytes));
    }

    #[test]
    fn base64url_reads_padded_and_unpadded_text_and_refuses_the_standard_alphabet() {
        for text in ["-_8=", "-_8"] {
            assert_eq!(
                super::decode(text, Alphabet::UrlSafe),
                Some(vec![0xfb, 0xff]),
                "{text:?}"
            );
        }
        for text in ["+/8=", "+/8", "Zg=", "Zh==", "-_8 ", "=-_8"] {
            assert_eq!(super::decode(text, Alphabet::UrlSafe), None, "{text:?}");
        }
    }

    #[test]
    fn a_json_member_is_a_base64url_string() {
        let payload = UrlPayload(Bytes::from_static(&[0xfb, 0xff, 0x00]));
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json, serde_json::json!("-_8A"));
        assert_eq!(serde_json::from_value::<UrlPayload>(json).unwrap(), payload);
        let error = serde_json::from_value::<UrlPayload>(serde_json::json!("+/8A")).unwrap_err();
        assert!(error.to_string().contains("base64url string"), "{error}");
    }

    proptest! {
        #[test]
        fn every_byte_string_round_trips(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let encoded = encode(&bytes);
            prop_assert_eq!(encoded.len() % 4, 0);
            prop_assert_eq!(decode(&encoded), Some(bytes.clone()));
            prop_assert_eq!(decode(encoded.trim_end_matches('=')), Some(bytes.clone()));
            let url = super::encode(&bytes, Alphabet::UrlSafe);
            prop_assert_eq!(&url, &encoded.replace('+', "-").replace('/', "_"));
            prop_assert_eq!(super::decode(&url, Alphabet::UrlSafe), Some(bytes.clone()));
            prop_assert_eq!(super::decode(url.trim_end_matches('='), Alphabet::UrlSafe), Some(bytes));
        }
    }
}
