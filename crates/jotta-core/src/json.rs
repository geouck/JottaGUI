//! Minimal JSON value tree, parser and serializer (no external deps).
//!
//! Good enough for machine-generated output from `jotta-cli ... --json`.
//! Strict enough to reject malformed input early instead of guessing.

use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(map) => map.get(key),
            Value::Array(items) => key.parse::<usize>().ok().and_then(|i| items.get(i)),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(v) => Some(v),
            _ => None,
        }
    }
    /// Walk a chain of keys, e.g. `v.path(&["account", "usage"])`.
    pub fn path(&self, keys: &[&str]) -> Option<&Value> {
        let mut cur = self;
        for k in keys {
            cur = cur.get(k)?;
        }
        Some(cur)
    }

    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    fn write_json(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    let _ = write!(out, "{}", *n as i64);
                } else {
                    let _ = write!(out, "{n}");
                }
            }
            Value::String(s) => write_escaped(out, s),
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    item.write_json(out);
                }
                out.push(']');
            }
            Value::Object(map) => {
                out.push('{');
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    write_escaped(out, k);
                    out.push(':');
                    v.write_json(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_escaped(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn parse(input: &str) -> Result<Value, String> {
    let mut p = Parser { bytes: input.as_bytes(), pos: 0 };
    p.skip_ws();
    let v = p.value()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(format!("trailing characters at offset {}", p.pos));
    }
    Ok(v)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
    fn expect(&mut self, b: u8) -> Result<(), String> {
        match self.peek() {
            Some(x) if x == b => { self.pos += 1; Ok(()) }
            other => Err(format!("expected {:?} at offset {}, found {:?}", b as char, self.pos, other.map(|x| x as char))),
        }
    }
    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') => self.lit("true", Value::Bool(true)),
            Some(b'f') => self.lit("false", Value::Bool(false)),
            Some(b'n') => self.lit("null", Value::Null),
            Some(b'-') | Some(b'0'..=b'9') => self.number(),
            other => Err(format!("unexpected {:?} at offset {}", other.map(|x| x as char), self.pos)),
        }
    }
    fn lit(&mut self, word: &str, val: Value) -> Result<Value, String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(val)
        } else {
            Err(format!("invalid literal at offset {}", self.pos))
        }
    }
    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') { self.pos += 1; }
        while matches!(self.peek(), Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')) {
            self.pos += 1;
        }
        let raw = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|e| e.to_string())?;
        raw.parse::<f64>().map(Value::Number).map_err(|e| format!("bad number `{raw}`: {e}"))
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut s = String::new();
        loop {
            let b = self.peek().ok_or("unterminated string")?;
            self.pos += 1;
            match b {
                b'"' => return Ok(s),
                b'\\' => {
                    let esc = self.peek().ok_or("unterminated escape")?;
                    self.pos += 1;
                    match esc {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'b' => s.push('\u{8}'),
                        b'f' => s.push('\u{c}'),
                        b'u' => {
                            let hex = self.bytes.get(self.pos..self.pos + 4)
                                .ok_or("bad \\u escape")?;
                            let code = u32::from_str_radix(
                                std::str::from_utf8(hex).map_err(|e| e.to_string())?, 16)
                                .map_err(|e| format!("bad \\u escape: {e}"))?;
                            self.pos += 4;
                            // Handle surrogate pairs (best effort).
                            if (0xD800..0xDC00).contains(&code) {
                                let hex2 = self.bytes.get(self.pos + 2..self.pos + 6)
                                    .ok_or("bad surrogate pair")?;
                                let low = u32::from_str_radix(
                                    std::str::from_utf8(hex2).map_err(|e| e.to_string())?, 16)
                                    .map_err(|e| format!("bad surrogate pair: {e}"))?;
                                self.pos += 6;
                                let combined = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                                s.push(char::from_u32(combined).unwrap_or('\u{fffd}'));
                            } else {
                                s.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                            }
                        }
                        other => return Err(format!("bad escape \\{}", other as char)),
                    }
                }
                _ => {
                    // Collect a full UTF-8 sequence.
                    let len = utf8_len(b);
                    let start = self.pos - 1;
                    let end = start + len;
                    let chunk = self.bytes.get(start..end).ok_or("truncated utf-8")?;
                    s.push_str(std::str::from_utf8(chunk).map_err(|e| e.to_string())?);
                    self.pos = end;
                }
            }
        }
    }
    fn array(&mut self) -> Result<Value, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') { self.pos += 1; return Ok(Value::Array(items)); }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => { self.pos += 1; }
                Some(b']') => { self.pos += 1; return Ok(Value::Array(items)); }
                other => return Err(format!("expected , or ] at offset {}, found {:?}", self.pos, other.map(|x| x as char))),
            }
        }
    }
    fn object(&mut self) -> Result<Value, String> {
        self.expect(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') { self.pos += 1; return Ok(Value::Object(map)); }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let val = self.value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => { self.pos += 1; }
                Some(b'}') => { self.pos += 1; return Ok(Value::Object(map)); }
                other => return Err(format!("expected , or }} at offset {}, found {:?}", self.pos, other.map(|x| x as char))),
            }
        }
    }
}

fn utf8_len(first: u8) -> usize {
    if first < 0x80 { 1 } else if first < 0xE0 { 2 } else if first < 0xF0 { 3 } else { 4 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_object() {
        let v = parse(r#"{"a":1,"b":[true,null,"x\n"],"c":{"d":1.5}}"#).unwrap();
        assert_eq!(v.get("a"), Some(&Value::Number(1.0)));
        assert_eq!(v.path(&["b", "2"]).and_then(|x| x.as_str()), Some("x\n"));
        assert_eq!(v.path(&["c", "d"]).and_then(|x| x.as_f64()), Some(1.5));
    }

    #[test]
    fn roundtrips() {
        let src = r#"{"name":"jotta","usage":{"used":1234.5,"quota":5368709120000}}"#;
        let v = parse(src).unwrap();
        assert_eq!(v.to_json(), parse(&v.to_json()).unwrap().to_json());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("{").is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("hello").is_err());
    }

    #[test]
    fn unicode_escapes() {
        let v = parse(r#""j\u00f8tta\ud83d\ude0e""#).unwrap();
        assert_eq!(v.as_str(), Some("jøtta😎"));
    }
}
