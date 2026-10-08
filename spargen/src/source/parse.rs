use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser as YamlParser, Tag};
use yaml_rust2::scanner::{Marker, TScalarStyle};
use yaml_rust2::Yaml;

use crate::diag::{
    Aborted, Code, Diagnostic, Diagnostics, FileId, JsonPointer, Loc, Provenance, Span,
};

use super::{Node, Number, SpannedKey, SpannedMap, SpannedValue};

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

/// Parse a YAML 1.2 document into a span-preserving [`SpannedValue`] tree.
///
/// YAML is restricted to the JSON-compatible subset OAS 3.1 prescribes: aliases expand to clones
/// of their anchored nodes, and constructs outside that subset (non-string keys, multiple
/// documents) are diagnosed. The low-level event API
/// (`yaml_rust2::parser::Parser` + a [`MarkedEventReceiver`]) is used instead of `YamlLoader` so
/// that each event's source [`Marker`] can be turned into a per-node span. Errors are reported
/// through `diags`.
pub(crate) fn parse_yaml(
    file: FileId,
    text: &str,
    diags: &mut Diagnostics,
) -> Result<SpannedValue, Aborted> {
    let positions = Positions::new(text);
    let mut sink = EventSink::default();
    let mut parser = YamlParser::new_from_str(text);
    if let Err(error) = parser.load(&mut sink, true) {
        let loc = positions.loc(error.marker().index());
        Diagnostic::error(
            Code::InvalidInput,
            Provenance::new(JsonPointer::root(), Some(Span::point(file, loc))),
        )
        .message(format!("malformed YAML: {error}"))
        .remedy("fix the YAML syntax before running spargen")
        .emit(diags);
        return Err(Aborted);
    }

    let doc_count = sink
        .events
        .iter()
        .filter(|(event, _)| matches!(event, Event::DocumentStart))
        .count();
    if doc_count != 1 {
        Diagnostic::error(
            Code::InvalidInput,
            Provenance::new(JsonPointer::root(), Some(root_span(file, &positions))),
        )
        .message("YAML input must contain exactly one document")
        .emit(diags);
        return Err(Aborted);
    }

    let mut builder = YamlBuilder {
        file,
        events: &sink.events,
        positions: &positions,
        idx: 0,
        anchors: std::collections::HashMap::new(),
    };
    builder.build(diags)
}

/// One step of the path from the document root to a duplicate key: an object member or an array
/// element. Both parsers collect the path innermost first while the failure unwinds, so a document
/// that parses pays nothing for it.
enum Segment {
    Key(String),
    Index(usize),
}

/// The pointer that `path`, innermost segment first, addresses.
fn pointer_of(path: &[Segment]) -> JsonPointer {
    path.iter()
        .rev()
        .fold(JsonPointer::root(), |pointer, segment| match segment {
            Segment::Key(name) => pointer.push(name),
            Segment::Index(index) => pointer.index(*index),
        })
}

/// The message `E022` carries in both parsers.
fn duplicate_key_message(name: &str) -> String {
    format!("duplicate object key `{name}`")
}

/// The remedy `E022` carries in both parsers.
const DUPLICATE_KEY_REMEDY: &str =
    "remove or rename the duplicate key so the object is unambiguous";

// JSON: hand-rolled span-tracking recursive-descent parser

/// Guard against unbounded recursion on pathological input (deeply nested `[[[…]]]`).
const MAX_JSON_DEPTH: u32 = 256;

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
        if depth > MAX_JSON_DEPTH {
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

// YAML: event-based span-tracking builder over the JSON-compatible subset

/// Collects `(event, marker)` pairs from `yaml_rust2`'s push parser so the tree can be built in a
/// second pass with lookahead to container-end markers.
#[derive(Default)]
struct EventSink {
    events: Vec<(Event, Marker)>,
}

impl MarkedEventReceiver for EventSink {
    fn on_event(&mut self, event: Event, marker: Marker) {
        self.events.push((event, marker));
    }
}

/// Builds a [`SpannedValue`] tree from a flat event stream, deriving each node's span from event
/// markers. Container spans run from their start event to their matching end event; scalar spans
/// cover the scalar text (best effort for multi-line/quoted forms).
struct YamlBuilder<'a> {
    file: FileId,
    events: &'a [(Event, Marker)],
    positions: &'a Positions,
    idx: usize,
    /// Anchor id → the node it labels, so aliases can be expanded exactly as `YamlLoader` does.
    /// Populated post-order as each anchored node completes.
    anchors: std::collections::HashMap<usize, SpannedValue>,
}

/// Why [`YamlBuilder::build_node`] stopped.
enum YamlFailure {
    /// The failure is already reported in the diagnostics.
    Reported(Aborted),
    /// A mapping declares `name` twice; the second occurrence sits at `span`. `path` runs from the
    /// document root to that key, innermost segment first, and each enclosing container prepends
    /// its own segment as the failure unwinds through it, before [`YamlBuilder::build`] reports it.
    DuplicateKey {
        path: Vec<Segment>,
        span: Span,
        name: String,
    },
}

impl YamlFailure {
    /// The failure as seen from the container one level up, which holds the failing node under
    /// `segment`.
    fn within(mut self, segment: Segment) -> Self {
        if let Self::DuplicateKey { path, .. } = &mut self {
            path.push(segment);
        }
        self
    }

    /// The failure as seen from a mapping whose key failed to build: a key has no pointer of its
    /// own, so a duplicate inside it is addressed by the mapping that holds the key.
    fn in_key(mut self) -> Self {
        if let Self::DuplicateKey { path, .. } = &mut self {
            path.clear();
        }
        self
    }
}

impl YamlBuilder<'_> {
    fn build(&mut self, diags: &mut Diagnostics) -> Result<SpannedValue, Aborted> {
        // Advance to the (single) document's first node event.
        while self.idx < self.events.len()
            && !matches!(self.events[self.idx].0, Event::DocumentStart)
        {
            self.idx += 1;
        }
        self.idx += 1; // step past DocumentStart onto the root node
        self.build_node(diags).map_err(|failure| match failure {
            YamlFailure::Reported(aborted) => aborted,
            YamlFailure::DuplicateKey { path, span, name } => {
                // `YamlLoader` rejected duplicate mapping keys; keep rejecting them (now with a
                // precise span at the second occurrence) rather than silently retaining a
                // duplicate.
                Diagnostic::error(
                    Code::DuplicateObjectKey,
                    Provenance::new(pointer_of(&path), Some(span)),
                )
                .message(duplicate_key_message(&name))
                .remedy(DUPLICATE_KEY_REMEDY)
                .emit(diags);
                Aborted
            }
        })
    }

    fn build_node(&mut self, diags: &mut Diagnostics) -> Result<SpannedValue, YamlFailure> {
        let events = self.events;
        let positions = self.positions;
        let (event, marker) = &events[self.idx];
        let start = positions.loc(marker.index());
        // The anchor id (0 = none) labelling this node, captured before it is built so the
        // completed node can be registered for later alias expansion.
        let anchor_id = match event {
            Event::Scalar(_, _, anchor, _) => *anchor,
            Event::SequenceStart(anchor, _) | Event::MappingStart(anchor, _) => *anchor,
            _ => 0,
        };
        let value = match event {
            Event::Scalar(value, style, _anchor, tag) => {
                let node = scalar_to_node(value, *style, tag);
                let end = scalar_end(positions, *style, value, marker.index());
                self.idx += 1;
                SpannedValue::new(
                    node,
                    Span {
                        file: self.file,
                        start,
                        end,
                    },
                )
            }
            Event::SequenceStart(..) => {
                self.idx += 1;
                let mut items = Vec::new();
                while !matches!(events[self.idx].0, Event::SequenceEnd) {
                    let item = self
                        .build_node(diags)
                        .map_err(|failure| failure.within(Segment::Index(items.len())))?;
                    items.push(item);
                }
                let end = positions.loc(events[self.idx].1.index());
                self.idx += 1; // consume SequenceEnd
                SpannedValue::new(
                    Node::Array(items),
                    Span {
                        file: self.file,
                        start,
                        end,
                    },
                )
            }
            Event::MappingStart(..) => {
                self.idx += 1;
                let mut map = SpannedMap::default();
                while !matches!(events[self.idx].0, Event::MappingEnd) {
                    let key = self.build_node(diags).map_err(YamlFailure::in_key)?;
                    let Node::String(name) = key.node else {
                        Diagnostic::error(
                            Code::InvalidInput,
                            Provenance::new(JsonPointer::root(), Some(key.span)),
                        )
                        .message("YAML object keys must be strings")
                        .emit(diags);
                        return Err(YamlFailure::Reported(Aborted));
                    };
                    if map.get(&name).is_some() {
                        return Err(YamlFailure::DuplicateKey {
                            path: vec![Segment::Key(name.clone())],
                            span: key.span,
                            name,
                        });
                    }
                    let value = self
                        .build_node(diags)
                        .map_err(|failure| failure.within(Segment::Key(name.clone())))?;
                    map.push(
                        SpannedKey {
                            name,
                            span: key.span,
                        },
                        value,
                    );
                }
                let end = positions.loc(events[self.idx].1.index());
                self.idx += 1; // consume MappingEnd
                SpannedValue::new(
                    Node::Object(map),
                    Span {
                        file: self.file,
                        start,
                        end,
                    },
                )
            }
            Event::Alias(id) => {
                // Expand the alias to a clone of its anchored node, mirroring `YamlLoader`:
                // aliases resolve to concrete values and so stay within the JSON-compatible
                // subset. An unknown anchor resolves to null, exactly as `YamlLoader` does.
                let id = *id;
                self.idx += 1;
                return Ok(self.anchors.get(&id).cloned().unwrap_or_else(|| {
                    SpannedValue::new(Node::Null, Span::point(self.file, start))
                }));
            }
            // The remaining events (stream/document delimiters, container ends) are consumed by
            // their openers and never begin a node.
            _ => {
                Diagnostic::error(
                    Code::InvalidInput,
                    Provenance::new(JsonPointer::root(), Some(Span::point(self.file, start))),
                )
                .message("unexpected YAML structure")
                .emit(diags);
                return Err(YamlFailure::Reported(Aborted));
            }
        };
        if anchor_id > 0 {
            self.anchors.insert(anchor_id, value.clone());
        }
        Ok(value)
    }
}

/// Interpret a YAML scalar exactly as `yaml_rust2`'s `YamlLoader` does (respecting quoting style
/// and explicit `!!`-tags), then map it onto a [`Node`]. Reusing the crate's own scalar typing
/// keeps parsed *values* identical to the previous `YamlLoader`-based path — only spans change.
fn scalar_to_node(value: &str, style: TScalarStyle, tag: &Option<Tag>) -> Node {
    let yaml = if style != TScalarStyle::Plain {
        Yaml::String(value.to_owned())
    } else if let Some(Tag { handle, suffix }) = tag {
        if handle == "tag:yaml.org,2002:" {
            match suffix.as_str() {
                "bool" => match value {
                    "true" | "True" | "TRUE" => Yaml::Boolean(true),
                    "false" | "False" | "FALSE" => Yaml::Boolean(false),
                    _ => Yaml::BadValue,
                },
                "int" => match value.parse::<i64>() {
                    Ok(parsed) => Yaml::Integer(parsed),
                    Err(_) => Yaml::BadValue,
                },
                "float" => {
                    if is_yaml_real(value) {
                        Yaml::Real(value.to_owned())
                    } else {
                        Yaml::BadValue
                    }
                }
                "null" => match value {
                    "~" | "null" => Yaml::Null,
                    _ => Yaml::BadValue,
                },
                _ => Yaml::String(value.to_owned()),
            }
        } else {
            Yaml::String(value.to_owned())
        }
    } else {
        Yaml::from_str(value)
    };
    yaml_scalar_to_node(yaml)
}

/// Map a scalar [`Yaml`] onto a [`Node`], preserving the previous path's number handling
/// (`Integer` → `Int`, `Real` → `Float`, out-of-range/`BadValue` fall through to `Null`).
fn yaml_scalar_to_node(yaml: Yaml) -> Node {
    match yaml {
        Yaml::Null | Yaml::BadValue => Node::Null,
        Yaml::Boolean(value) => Node::Bool(value),
        Yaml::Integer(value) => Node::Number(Number::Int(value)),
        Yaml::Real(value) => Node::Number(Number::Float(value.parse().unwrap_or(0.0))),
        Yaml::String(value) => Node::String(value),
        // Scalars never carry these variants; treat defensively as null.
        Yaml::Array(_) | Yaml::Hash(_) | Yaml::Alias(_) => Node::Null,
    }
}

/// Mirror `yaml_rust2`'s (private) `parse_f64` acceptance test for the explicit `!!float` tag.
fn is_yaml_real(value: &str) -> bool {
    matches!(
        value,
        ".inf"
            | ".Inf"
            | ".INF"
            | "+.inf"
            | "+.Inf"
            | "+.INF"
            | "-.inf"
            | "-.Inf"
            | "-.INF"
            | ".nan"
            | ".NaN"
            | ".NAN"
    ) || (value.bytes().any(|byte| byte.is_ascii_digit()) && value.parse::<f64>().is_ok())
}

/// A best-effort end position for a scalar: for a single-line scalar it spans the scalar text
/// (plus surrounding quotes, if any); multi-line/block scalars collapse to a point at the start.
fn scalar_end(positions: &Positions, style: TScalarStyle, value: &str, start_index: usize) -> Loc {
    if value.contains('\n') {
        return positions.loc(start_index);
    }
    let quotes = match style {
        TScalarStyle::SingleQuoted | TScalarStyle::DoubleQuoted => 2,
        _ => 0,
    };
    positions.loc(start_index + value.chars().count() + quotes)
}

// Shared position bookkeeping

/// A precomputed map from a source character index to its [`Loc`] (byte offset + 1-based
/// line/column). `yaml_rust2` reports positions as character indices, so this converts them to
/// the byte-offset-bearing [`Loc`] the diagnostic model uses. The trailing sentinel entry maps
/// the end-of-input index.
struct Positions {
    byte: Vec<usize>,
    line: Vec<u32>,
    col: Vec<u32>,
}

impl Positions {
    fn new(text: &str) -> Self {
        let mut byte = Vec::with_capacity(text.len() + 1);
        let mut line = Vec::with_capacity(text.len() + 1);
        let mut col = Vec::with_capacity(text.len() + 1);
        let mut current_line = 1u32;
        let mut current_col = 1u32;
        for (offset, ch) in text.char_indices() {
            byte.push(offset);
            line.push(current_line);
            col.push(current_col);
            if ch == '\n' {
                current_line += 1;
                current_col = 1;
            } else {
                current_col += 1;
            }
        }
        byte.push(text.len());
        line.push(current_line);
        col.push(current_col);
        Self { byte, line, col }
    }

    fn loc(&self, char_index: usize) -> Loc {
        let index = char_index.min(self.byte.len() - 1);
        Loc {
            line: self.line[index],
            col: self.col[index],
            offset: self.byte[index],
        }
    }
}

fn root_span(file: FileId, positions: &Positions) -> Span {
    Span {
        file,
        start: Loc {
            line: 1,
            col: 1,
            offset: 0,
        },
        end: positions.loc(positions.byte.len() - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_json_ok(text: &str) -> SpannedValue {
        let mut diags = Diagnostics::default();
        let value = parse_json(FileId(0), text, &mut diags).expect("json parses");
        assert!(!diags.has_errors(), "unexpected diagnostics: {diags:?}");
        value
    }

    fn parse_yaml_ok(text: &str) -> SpannedValue {
        let mut diags = Diagnostics::default();
        let value = parse_yaml(FileId(0), text, &mut diags).expect("yaml parses");
        assert!(!diags.has_errors(), "unexpected diagnostics: {diags:?}");
        value
    }

    #[test]
    fn json_nested_node_span_is_precise() {
        // A property three levels deep sits on a known line/column; its span must point there,
        // not at line 1.
        let text = "{\n  \"a\": {\n    \"b\": {\n      \"c\": 42\n    }\n  }\n}\n";
        let root = parse_json_ok(text);
        let c = root
            .get("a")
            .and_then(|value| value.get("b"))
            .and_then(|value| value.get("c"))
            .expect("nested `c` exists");
        // `      "c": 42` is on line 4; the value `42` begins at column 12.
        assert_eq!(c.span().start.line, 4, "value line");
        assert_eq!(c.span().start.col, 12, "value column");
        assert_eq!(c.node, Node::Number(Number::Int(42)));

        // The key span points at the key token, also on line 4 (column 7 for the opening quote).
        let b = root.get("a").and_then(|value| value.get("b")).unwrap();
        let (key, _) = b.as_object().unwrap().iter().next().unwrap();
        assert_eq!(key.name, "c");
        assert_eq!(key.span.start.line, 4);
        assert_eq!(key.span.start.col, 7);
    }

    #[test]
    fn json_number_classification_matches_serde() {
        let root =
            parse_json_ok("{\"i\": -7, \"u\": 18446744073709551615, \"f\": 1.5, \"e\": 1e3}");
        assert_eq!(root.get("i").unwrap().node, Node::Number(Number::Int(-7)));
        assert_eq!(
            root.get("u").unwrap().node,
            Node::Number(Number::UInt(18446744073709551615))
        );
        assert_eq!(
            root.get("f").unwrap().node,
            Node::Number(Number::Float(1.5))
        );
        assert_eq!(
            root.get("e").unwrap().node,
            Node::Number(Number::Float(1000.0))
        );
    }

    #[test]
    fn json_string_escapes_and_surrogate_pairs() {
        let root = parse_json_ok(r#"{"s": "line\n\t\"q\"A😀"}"#);
        assert_eq!(
            root.get("s").unwrap().as_str(),
            Some("line\n\t\"q\"A\u{1F600}"),
        );
    }

    #[test]
    fn json_malformed_error_span_points_at_error_line() {
        let mut diags = Diagnostics::default();
        // The value for `b` is missing; the error is detected on line 3.
        let text = "{\n  \"a\": 1,\n  \"b\":\n}\n";
        let outcome = parse_json(FileId(0), text, &mut diags);
        assert!(outcome.is_err());
        let diag = diags
            .items()
            .iter()
            .find(|diag| diag.code == Code::InvalidInput)
            .expect("invalid-input diagnostic");
        let span = diag.span.expect("error span");
        assert_eq!(span.start.line, 4, "error reported on line 4: {diag:?}");
    }

    fn parse_json_err_code(text: &str) -> Code {
        let mut diags = Diagnostics::default();
        assert!(
            parse_json(FileId(0), text, &mut diags).is_err(),
            "expected `{text}` to be rejected"
        );
        diags
            .items()
            .iter()
            .map(|diag| diag.code)
            .next()
            .unwrap_or_else(|| panic!("no diagnostic for `{text}`"))
    }

    #[test]
    fn json_rejects_malformed_numbers_like_serde() {
        // Leading zeros, a fraction/exponent with no digit, and an overflow-to-infinity literal
        // are all malformed and must be rejected.
        for bad in [
            "01",
            "00",
            "-01",
            "1.",
            "1.e5",
            "1e",
            "1e+",
            "-",
            "1e999",
            "[01]",
            "{\"a\": 1.}",
        ] {
            assert_eq!(
                parse_json_err_code(bad),
                Code::InvalidInput,
                "`{bad}` should be rejected as malformed"
            );
        }
    }

    #[test]
    fn json_still_accepts_valid_numbers() {
        // The tightened grammar must not reject well-formed numbers.
        let root = parse_json_ok("{\"a\": 0, \"b\": -0, \"c\": 0.5, \"d\": 1e3, \"e\": -12.5e-2}");
        assert_eq!(root.get("a").unwrap().node, Node::Number(Number::Int(0)));
        assert_eq!(root.get("b").unwrap().node, Node::Number(Number::Int(0)));
        assert_eq!(
            root.get("c").unwrap().node,
            Node::Number(Number::Float(0.5))
        );
    }

    #[test]
    fn json_rejects_duplicate_object_key_at_the_second_occurrence() {
        let mut diags = Diagnostics::default();
        // Two `"type"` keys on line 2; the second (the duplicate) opening quote is at column 21.
        let text = "{\n  \"type\": \"string\", \"type\": \"integer\"\n}\n";
        assert!(parse_json(FileId(0), text, &mut diags).is_err());
        let diag = diags
            .items()
            .iter()
            .find(|diag| diag.code == Code::DuplicateObjectKey)
            .expect("duplicate-key diagnostic");
        let span = diag.span.expect("duplicate-key span");
        assert_eq!(span.start.line, 2, "{diag:?}");
        assert_eq!(span.start.col, 21, "{diag:?}");
        assert_eq!(diag.pointer.as_str(), "/type", "{diag:?}");
    }

    /// The duplicate key's pointer, or `None` when `diags` holds no `E022`.
    fn duplicate_key_pointer(diags: &Diagnostics) -> Option<String> {
        diags
            .items()
            .iter()
            .find(|diag| diag.code == Code::DuplicateObjectKey)
            .map(|diag| diag.pointer.as_str().to_owned())
    }

    #[test]
    fn duplicate_object_key_points_at_the_key_through_objects_and_arrays() {
        // The path runs through an object member, an array element, and a member whose name
        // needs RFC 6901 escaping, so each kind of segment and the escape are reached.
        let json = r#"{"a": {"x/y": [0, {"k": 1, "k": 2}]}}"#;
        let yaml = "a:\n  x/y:\n    - 0\n    - k: 1\n      k: 2\n";
        let mut diags = Diagnostics::default();
        assert!(parse_json(FileId(0), json, &mut diags).is_err());
        assert_eq!(
            duplicate_key_pointer(&diags).as_deref(),
            Some("/a/x~1y/1/k")
        );
        let mut diags = Diagnostics::default();
        assert!(parse_yaml(FileId(0), yaml, &mut diags).is_err());
        assert_eq!(
            duplicate_key_pointer(&diags).as_deref(),
            Some("/a/x~1y/1/k")
        );
    }

    #[test]
    fn a_json_syntax_error_keeps_the_root_pointer() {
        // Only a duplicate key collects a path; a syntax error stays document-level.
        let mut diags = Diagnostics::default();
        assert!(parse_json(FileId(0), r#"{"a": [1, }"#, &mut diags).is_err());
        let diag = diags
            .items()
            .iter()
            .find(|diag| diag.code == Code::InvalidInput)
            .expect("syntax diagnostic");
        assert_eq!(diag.pointer.as_str(), "", "{diag:?}");
    }

    #[test]
    fn a_duplicate_inside_a_yaml_complex_key_points_at_the_mapping_holding_the_key() {
        // A key has no pointer of its own, so the mapping that holds it addresses the duplicate.
        let mut diags = Diagnostics::default();
        let text = "a:\n  ? {k: 1, k: 2}\n  : v\n";
        assert!(parse_yaml(FileId(0), text, &mut diags).is_err());
        assert_eq!(duplicate_key_pointer(&diags).as_deref(), Some("/a"));
    }

    #[test]
    fn yaml_rejects_duplicate_mapping_key() {
        let mut diags = Diagnostics::default();
        // `a` appears twice; the duplicate is on line 2.
        let text = "a: 1\na: 2\n";
        assert!(parse_yaml(FileId(0), text, &mut diags).is_err());
        let diag = diags
            .items()
            .iter()
            .find(|diag| diag.code == Code::DuplicateObjectKey)
            .expect("duplicate-key diagnostic");
        assert_eq!(diag.span.expect("span").start.line, 2, "{diag:?}");
        assert_eq!(diag.pointer.as_str(), "/a", "{diag:?}");
    }

    #[test]
    fn yaml_nested_node_span_is_precise() {
        // `c` is three levels deep on line 4 (1-based; line 1 is the leading newline).
        let text = "\na:\n  b:\n    c: hello\n";
        let root = parse_yaml_ok(text);
        let c = root
            .get("a")
            .and_then(|value| value.get("b"))
            .and_then(|value| value.get("c"))
            .expect("nested `c` exists");
        assert_eq!(c.node, Node::String("hello".to_owned()));
        assert_eq!(c.span().start.line, 4, "value line");
        // `    c: hello` — the value `hello` starts at column 8.
        assert_eq!(c.span().start.col, 8, "value column");

        // The key `c` is at column 5 on the same line.
        let b = root.get("a").and_then(|value| value.get("b")).unwrap();
        let (key, _) = b.as_object().unwrap().iter().next().unwrap();
        assert_eq!(key.name, "c");
        assert_eq!(key.span.start.line, 4);
        assert_eq!(key.span.start.col, 5);
    }

    #[test]
    fn yaml_alias_is_expanded_like_yamlloader() {
        // The previous `YamlLoader`-based path expanded aliases into concrete values; that
        // behavior is preserved so parsed values stay identical (only spans changed). The alias
        // `*anchor` resolves to a clone of the anchored `1` rather than being rejected.
        let root = parse_yaml_ok("a: &anchor 1\nb: *anchor\n");
        assert_eq!(root.get("a").unwrap().node, Node::Number(Number::Int(1)));
        assert_eq!(root.get("b").unwrap().node, Node::Number(Number::Int(1)));
    }

    #[test]
    fn yaml_multibyte_column_counts_scalar_values() {
        // A multibyte key precedes the value on the same line; the value column must count
        // characters, not bytes.
        let text = "\"café\": tail\n";
        let root = parse_yaml_ok(text);
        let value = root.get("café").expect("multibyte key present");
        assert_eq!(value.as_str(), Some("tail"));
        // `"café": tail` — 7 characters (`"`,`c`,`a`,`f`,`é`,`"`,`:`) then a space, so `tail`
        // starts at column 9.
        assert_eq!(value.span().start.col, 9);
    }
}
