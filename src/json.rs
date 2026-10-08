use std::collections::BTreeMap;

use crate::error::{Error, Result};

// Deeply nested input would recurse until the stack overflows, which aborts the
// process without unwinding and leaves the terminal in raw mode. serde_json uses
// the same default limit.
const MAX_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            JsonValue::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            JsonValue::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonValue::Number(value) if value.is_finite() && value.fract() == 0.0 => {
                Some(*value as i64)
            }
            _ => None,
        }
    }

    pub fn object_get<'a>(&'a self, key: &str) -> Option<&'a JsonValue> {
        self.as_object()?.get(key)
    }
}

pub fn parse(input: &str) -> Result<JsonValue> {
    let mut parser = Parser {
        input,
        pos: 0,
        depth: 0,
    };
    let value = parser.parse_value()?;
    parser.skip_ws();
    if !parser.is_eof() {
        return Err(Error::new("unexpected trailing JSON input"));
    }
    Ok(value)
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn next_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek_char(), Some(' ' | '\n' | '\r' | '\t')) {
            let _ = self.next_char();
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue> {
        self.skip_ws();
        let container = matches!(self.peek_char(), Some('{' | '['));
        if container {
            self.depth += 1;
            if self.depth > MAX_DEPTH {
                return Err(Error::new("JSON nesting is too deep"));
            }
        }
        let result = match self.peek_char() {
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => self.parse_string().map(JsonValue::String),
            Some('t') => self.parse_true(),
            Some('f') => self.parse_false(),
            Some('n') => self.parse_null(),
            Some('-') | Some('0'..='9') => self.parse_number(),
            Some(other) => Err(Error::new(format!("unexpected JSON token: {other}"))),
            None => Err(Error::new("unexpected end of JSON input")),
        };
        if container {
            self.depth -= 1;
        }
        result
    }

    fn parse_object(&mut self) -> Result<JsonValue> {
        self.expect('{')?;
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.consume('}') {
            return Ok(JsonValue::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(':')?;
            let value = self.parse_value()?;
            map.insert(key, value);
            self.skip_ws();
            if self.consume('}') {
                break;
            }
            self.expect(',')?;
        }
        Ok(JsonValue::Object(map))
    }

    fn parse_array(&mut self) -> Result<JsonValue> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.consume(']') {
            return Ok(JsonValue::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            if self.consume(']') {
                break;
            }
            self.expect(',')?;
        }
        Ok(JsonValue::Array(items))
    }

    fn parse_string(&mut self) -> Result<String> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            let ch = self
                .next_char()
                .ok_or_else(|| Error::new("unterminated JSON string"))?;
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let escaped = self
                        .next_char()
                        .ok_or_else(|| Error::new("unterminated JSON escape"))?;
                    match escaped {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{0008}'),
                        'f' => out.push('\u{000c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let code = self.parse_hex4()?;
                            if (0xD800..=0xDBFF).contains(&code) {
                                if !(self.consume('\\') && self.consume('u')) {
                                    return Err(Error::new(
                                        "high Unicode surrogate is missing a low surrogate",
                                    ));
                                }
                                let low = self.parse_hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err(Error::new("invalid Unicode surrogate pair"));
                                }
                                let high = (code - 0xD800) as u32;
                                let low = (low - 0xDC00) as u32;
                                let scalar = 0x10000 + ((high << 10) | low);
                                let ch = char::from_u32(scalar)
                                    .ok_or_else(|| Error::new("invalid Unicode escape"))?;
                                out.push(ch);
                            } else if let Some(ch) = char::from_u32(code as u32) {
                                out.push(ch);
                            } else {
                                return Err(Error::new("invalid Unicode escape"));
                            }
                        }
                        other => return Err(Error::new(format!("invalid JSON escape: {other}"))),
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u16> {
        let mut value = 0u16;
        for _ in 0..4 {
            let ch = self
                .next_char()
                .ok_or_else(|| Error::new("unterminated Unicode escape"))?;
            value = (value << 4)
                | ch.to_digit(16)
                    .ok_or_else(|| Error::new("invalid Unicode escape"))? as u16;
        }
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<JsonValue> {
        let start = self.pos;
        self.consume('-');
        self.consume_digits();
        if self.consume('.') {
            self.consume_digits();
        }
        if matches!(self.peek_char(), Some('e' | 'E')) {
            let _ = self.next_char();
            let _ = self.consume('+');
            let _ = self.consume('-');
            self.consume_digits();
        }
        let text = &self.input[start..self.pos];
        let value = text
            .parse::<f64>()
            .map_err(|_| Error::new(format!("invalid JSON number: {text}")))?;
        Ok(JsonValue::Number(value))
    }

    fn parse_true(&mut self) -> Result<JsonValue> {
        self.expect_str("true")?;
        Ok(JsonValue::Bool(true))
    }

    fn parse_false(&mut self) -> Result<JsonValue> {
        self.expect_str("false")?;
        Ok(JsonValue::Bool(false))
    }

    fn parse_null(&mut self) -> Result<JsonValue> {
        self.expect_str("null")?;
        Ok(JsonValue::Null)
    }

    fn consume_digits(&mut self) {
        while matches!(self.peek_char(), Some('0'..='9')) {
            let _ = self.next_char();
        }
    }

    fn expect(&mut self, expected: char) -> Result<()> {
        match self.next_char() {
            Some(ch) if ch == expected => Ok(()),
            Some(ch) => Err(Error::new(format!("expected '{expected}', got '{ch}'"))),
            None => Err(Error::new(format!("expected '{expected}', got EOF"))),
        }
    }

    fn consume(&mut self, expected: char) -> bool {
        if self.peek_char() == Some(expected) {
            let _ = self.next_char();
            true
        } else {
            false
        }
    }

    fn expect_str(&mut self, expected: &str) -> Result<()> {
        for ch in expected.chars() {
            self.expect(ch)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_nesting_is_rejected_instead_of_overflowing_the_stack() {
        let too_deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        assert!(parse(&too_deep).is_err());
        let acceptable = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(&acceptable).is_ok());
    }
}
