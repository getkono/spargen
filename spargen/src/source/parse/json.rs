//! JSON: a hand-rolled span-tracking recursive-descent parser.

use crate::diag::{
    Aborted, Code, Diagnostic, Diagnostics, FileId, JsonPointer, Loc, Provenance, Span,
};

use super::super::{Node, Number, SpannedKey, SpannedMap, SpannedValue};
use super::{duplicate_key_message, pointer_of, Segment, DUPLICATE_KEY_REMEDY, MAX_NESTING_DEPTH};

/// Parse a JSON document into a span-preserving [`SpannedValue`] tree.
///
/// A hand-rolled recursive-descent parser tracks line/column/offset as it scans, so every node
/// and every object key carries its precise `[start, end)` source range rather than the whole
/// file. Malformed input is reported through `diags` (with a span at the error location) rather
/// than by panic; a fatal parse error returns [`Aborted`].
pub(crate) fn parse_json(
    file: FileId,
    text: &str,
    diags: &mut Diagnostics,
) -> Result<SpannedValue, Aborted> {
    let mut parser = JsonParser::new(file, text);
    match parser.parse_document() {
        Ok(value) => Ok(value),
        Err(error) => {
            // Syntax errors are wrapped with a `malformed JSON: … at line/column` prefix;
            // structural errors that carry their own stable code (e.g. a duplicate key) render
            // their message verbatim.
            let message = if error.code == Code::InvalidInput {
                format!(
                    "malformed JSON: {} at line {} column {}",
                    error.message, error.span.start.line, error.span.start.col
                )
            } else {
                error.message
            };
            let pointer = error
                .path
                .as_deref()
                .map_or_else(JsonPointer::root, pointer_of);
            Diagnostic::error(error.code, Provenance::new(pointer, Some(error.span)))
                .message(message)
                .remedy(error.remedy)
                .emit(diags);
            Err(Aborted)
        }
    }
}

/// A JSON parse failure, carrying the precise source span it occurred at, the stable diagnostic
/// code to raise, and the message/remedy to render. Most failures are syntax errors (`E011`
/// `InvalidInput`); a duplicate key raises `E022` instead.
struct JsonError {
    span: Span,
    code: Code,
    message: String,
    remedy: &'static str,
    /// The path from the document root to the duplicate key, innermost segment first, for a
    /// duplicate key; `None` for a syntax error, which is reported at the root pointer.
    path: Option<Vec<Segment>>,
}

impl JsonError {
    /// The error as seen from the container one level up, which holds the failing value under
    /// `segment`. Each container the error unwinds through prepends its own segment, so the path
    /// is built only on failure.
    fn within(mut self, segment: Segment) -> Self {
        if let Some(path) = &mut self.path {
            path.push(segment);
        }
        self
    }
}

/// A cursor over the raw bytes of a JSON document that maintains line/column/offset so it can
/// stamp a precise [`Loc`] onto every node it produces.
struct JsonParser<'a> {
    file: FileId,
    bytes: &'a [u8],
    text: &'a str,
    /// Byte offset of the cursor.
    pos: usize,
    /// 1-based line at the cursor.
    line: u32,
    /// 1-based column (in Unicode scalar values) at the cursor.
    col: u32,
}

impl<'a> JsonParser<'a> {
    fn new(file: FileId, text: &'a str) -> Self {
        Self {
            file,
            bytes: text.as_bytes(),
            text,
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn loc(&self) -> Loc {
        Loc {
            line: self.line,
            col: self.col,
            offset: self.pos,
        }
    }

    fn span_from(&self, start: Loc) -> Span {
        Span {
            file: self.file,
            start,
            end: self.loc(),
        }
    }

    /// A syntax error (`E011`) at the current cursor position.
    fn error(&self, message: impl Into<String>) -> JsonError {
        self.error_at(self.loc(), message)
    }

    /// A syntax error (`E011`) at a specific location (e.g. the start of a bad number token).
    fn error_at(&self, loc: Loc, message: impl Into<String>) -> JsonError {
        JsonError {
            span: Span::point(self.file, loc),
            code: Code::InvalidInput,
            message: message.into(),
            remedy: "fix the JSON syntax before running spargen",
            path: None,
        }
    }

    /// A duplicate-object-key error (`E022`) pointed at the offending (second) key, whose pointer
    /// the enclosing containers complete as the error unwinds through them.
    fn duplicate_key_error(&self, key_span: Span, name: &str) -> JsonError {
        JsonError {
            span: key_span,
            code: Code::DuplicateObjectKey,
            message: duplicate_key_message(name),
            remedy: DUPLICATE_KEY_REMEDY,
            path: Some(vec![Segment::Key(name.to_owned())]),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// Advance one byte, updating line/column. Columns count Unicode scalar values: a UTF-8
    /// continuation byte (`0b10xx_xxxx`) does not begin a new character and so does not advance
    /// the column.
    fn bump(&mut self) {
        if let Some(&byte) = self.bytes.get(self.pos) {
            self.pos += 1;
            if byte == b'\n' {
                self.line += 1;
                self.col = 1;
            } else if byte & 0xC0 != 0x80 {
                self.col += 1;
            }
        }
    }

    fn skip_ws(&mut self) {
        while let Some(byte) = self.peek() {
            match byte {
                b' ' | b'\t' | b'\n' | b'\r' => self.bump(),
                _ => break,
            }
        }
    }

    fn parse_document(&mut self) -> Result<SpannedValue, JsonError> {
        self.skip_ws();
        if self.peek().is_none() {
            return Err(self.error("unexpected end of input"));
        }
        let value = self.parse_value(0)?;
        self.skip_ws();
        if self.pos != self.bytes.len() {
            return Err(self.error("trailing characters after JSON value"));
        }
        Ok(value)
    }

    fn parse_value(&mut self, depth: u32) -> Result<SpannedValue, JsonError> {
        if depth > MAX_NESTING_DEPTH {
            return Err(self.error("JSON nesting is too deep"));
        }
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(depth),
            Some(b'[') => self.parse_array(depth),
            Some(b'"') => {
                let start = self.loc();
                let value = self.parse_string()?;
                Ok(SpannedValue::new(
                    Node::String(value),
                    self.span_from(start),
                ))
            }
            Some(b't') | Some(b'f') => self.parse_bool(),
            Some(b'n') => self.parse_null(),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(_) => Err(self.error("expected a JSON value")),
            None => Err(self.error("unexpected end of input")),
        }
    }

    fn parse_object(&mut self, depth: u32) -> Result<SpannedValue, JsonError> {
        let start = self.loc();
        self.bump(); // consume '{'
        let mut map = SpannedMap::default();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.bump();
            return Ok(SpannedValue::new(Node::Object(map), self.span_from(start)));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a string key in object"));
            }
            let key_start = self.loc();
            let name = self.parse_string()?;
            let key_span = self.span_from(key_start);
            if map.get(&name).is_some() {
                return Err(self.duplicate_key_error(key_span, &name));
            }
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.error("expected ':' after object key"));
            }
            self.bump(); // consume ':'
            let value = self
                .parse_value(depth + 1)
                .map_err(|error| error.within(Segment::Key(name.clone())))?;
            map.push(
                SpannedKey {
                    name,
                    span: key_span,
                },
                value,
            );
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.bump(),
                Some(b'}') => {
                    self.bump();
                    break;
                }
                Some(_) => return Err(self.error("expected ',' or '}' in object")),
                None => return Err(self.error("unterminated object")),
            }
        }
        Ok(SpannedValue::new(Node::Object(map), self.span_from(start)))
    }

    fn parse_array(&mut self, depth: u32) -> Result<SpannedValue, JsonError> {
        let start = self.loc();
        self.bump(); // consume '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.bump();
            return Ok(SpannedValue::new(Node::Array(items), self.span_from(start)));
        }
        loop {
            let value = self
                .parse_value(depth + 1)
                .map_err(|error| error.within(Segment::Index(items.len())))?;
            items.push(value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.bump(),
                Some(b']') => {
                    self.bump();
                    break;
                }
                Some(_) => return Err(self.error("expected ',' or ']' in array")),
                None => return Err(self.error("unterminated array")),
            }
        }
        Ok(SpannedValue::new(Node::Array(items), self.span_from(start)))
    }

    /// Parse a JSON string, with the cursor positioned on the opening quote. Returns the decoded
    /// (unescaped) contents; the cursor ends just past the closing quote.
    fn parse_string(&mut self) -> Result<String, JsonError> {
        self.bump(); // consume opening '"'
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated string")),
                Some(b'"') => {
                    self.bump();
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.bump();
                    match self.peek() {
                        Some(b'"') => {
                            out.push('"');
                            self.bump();
                        }
                        Some(b'\\') => {
                            out.push('\\');
                            self.bump();
                        }
                        Some(b'/') => {
                            out.push('/');
                            self.bump();
                        }
                        Some(b'b') => {
                            out.push('\u{0008}');
                            self.bump();
                        }
                        Some(b'f') => {
                            out.push('\u{000C}');
                            self.bump();
                        }
                        Some(b'n') => {
                            out.push('\n');
                            self.bump();
                        }
                        Some(b'r') => {
                            out.push('\r');
                            self.bump();
                        }
                        Some(b't') => {
                            out.push('\t');
                            self.bump();
                        }
                        Some(b'u') => {
                            self.bump();
                            let ch = self.parse_unicode_escape()?;
                            out.push(ch);
                        }
                        _ => return Err(self.error("invalid escape sequence in string")),
                    }
                }
                Some(byte) if byte < 0x20 => {
                    return Err(self.error("unescaped control character in string"))
                }
                Some(_) => {
                    // Copy a whole UTF-8 scalar value verbatim: the lead byte plus any
                    // continuation bytes.
                    let char_start = self.pos;
                    self.bump();
                    while matches!(self.peek(), Some(byte) if byte & 0xC0 == 0x80) {
                        self.bump();
                    }
                    out.push_str(&self.text[char_start..self.pos]);
                }
            }
        }
    }

    /// Parse a `\u`-escape (the cursor is just past the `u`), resolving UTF-16 surrogate pairs.
    fn parse_unicode_escape(&mut self) -> Result<char, JsonError> {
        let high = self.parse_hex4()?;
        if (0xD800..=0xDBFF).contains(&high) {
            if self.peek() != Some(b'\\') {
                return Err(self.error("unpaired high surrogate in \\u escape"));
            }
            self.bump();
            if self.peek() != Some(b'u') {
                return Err(self.error("expected a low surrogate after a high surrogate"));
            }
            self.bump();
            let low = self.parse_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err(self.error("invalid low surrogate in \\u escape"));
            }
            let code = 0x1_0000 + ((high - 0xD800) << 10) + (low - 0xDC00);
            char::from_u32(code).ok_or_else(|| self.error("invalid unicode escape"))
        } else if (0xDC00..=0xDFFF).contains(&high) {
            Err(self.error("unexpected low surrogate in \\u escape"))
        } else {
            char::from_u32(high).ok_or_else(|| self.error("invalid unicode escape"))
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, JsonError> {
        let mut value = 0u32;
        for _ in 0..4 {
            match self.peek() {
                Some(byte) => {
                    let digit = match byte {
                        b'0'..=b'9' => u32::from(byte - b'0'),
                        b'a'..=b'f' => u32::from(byte - b'a' + 10),
                        b'A'..=b'F' => u32::from(byte - b'A' + 10),
                        _ => return Err(self.error("invalid hex digit in \\u escape")),
                    };
                    value = value * 16 + digit;
                    self.bump();
                }
                None => return Err(self.error("unterminated \\u escape")),
            }
        }
        Ok(value)
    }

    fn parse_bool(&mut self) -> Result<SpannedValue, JsonError> {
        let start = self.loc();
        if self.consume_keyword(b"true") {
            Ok(SpannedValue::new(Node::Bool(true), self.span_from(start)))
        } else if self.consume_keyword(b"false") {
            Ok(SpannedValue::new(Node::Bool(false), self.span_from(start)))
        } else {
            Err(self.error("invalid literal"))
        }
    }

    fn parse_null(&mut self) -> Result<SpannedValue, JsonError> {
        let start = self.loc();
        if self.consume_keyword(b"null") {
            Ok(SpannedValue::new(Node::Null, self.span_from(start)))
        } else {
            Err(self.error("invalid literal"))
        }
    }

    fn consume_keyword(&mut self, keyword: &[u8]) -> bool {
        if self.bytes[self.pos..].starts_with(keyword) {
            for _ in 0..keyword.len() {
                self.bump();
            }
            true
        } else {
            false
        }
    }

    /// Parse a number, enforcing the JSON grammar (`-? int frac? exp?`) so malformed literals are
    /// rejected exactly as `serde_json` rejects them: a leading zero may not be followed by another
    /// digit (`01`), a `.` must be followed by a digit (`1.`), an exponent must have a digit
    /// (`1e`), and a literal that overflows to a non-finite `f64` (`1e999`) is out of range.
    fn parse_number(&mut self) -> Result<SpannedValue, JsonError> {
        let start = self.loc();
        let start_offset = self.pos;
        if self.peek() == Some(b'-') {
            self.bump();
        }
        // Integer part: a lone `0`, or a non-zero digit followed by any digits.
        match self.peek() {
            Some(b'0') => {
                self.bump();
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.error_at(start, "number must not have a leading zero"));
                }
            }
            Some(b'1'..=b'9') => {
                self.bump();
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.bump();
                }
            }
            _ => return Err(self.error("expected a digit in number")),
        }
        let mut is_float = false;
        // Fraction: `.` then at least one digit.
        if self.peek() == Some(b'.') {
            is_float = true;
            self.bump();
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("expected a digit after '.' in number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
        }
        // Exponent: `e`/`E`, optional sign, then at least one digit.
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.bump();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.bump();
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("expected a digit in number exponent"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
        }
        let slice = &self.text[start_offset..self.pos];
        // Mirror `serde_json`'s classification: an integer literal is an `i64` when it fits, else a
        // `u64` when it fits, else a float; anything with a fraction or exponent is a float.
        let number = if is_float {
            Number::Float(self.parse_f64(slice, start)?)
        } else if let Ok(value) = slice.parse::<i64>() {
            Number::Int(value)
        } else if let Ok(value) = slice.parse::<u64>() {
            Number::UInt(value)
        } else {
            Number::Float(self.parse_f64(slice, start)?)
        };
        Ok(SpannedValue::new(
            Node::Number(number),
            self.span_from(start),
        ))
    }

    /// Parse a numeric slice as `f64`, rejecting a value that overflows to a non-finite float
    /// (e.g. `1e999`) as out-of-range — as `serde_json` does.
    fn parse_f64(&self, slice: &str, start: Loc) -> Result<f64, JsonError> {
        match slice.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(value),
            Ok(_) => Err(self.error_at(start, format!("number `{slice}` is out of range"))),
            Err(_) => Err(self.error_at(start, format!("invalid number literal `{slice}`"))),
        }
    }
}
