//! Bencode decoder. Never panics on malformed input.

use std::collections::BTreeMap;

use super::Value;
use crate::error::{Error, Result};

/// Decode a complete bencode value. Trailing bytes are an error.
pub fn decode(data: &[u8]) -> Result<Value> {
    let (v, consumed) = decode_prefix(data)?;
    if consumed != data.len() {
        return Err(Error::TrailingBytes {
            unused: data.len() - consumed,
        });
    }
    Ok(v)
}

/// Decode one bencode value from the start of `data`, returning the value and
/// the number of bytes consumed. Extra trailing bytes are allowed (used by
/// ut_metadata messages that append a payload after the bencode dict).
pub fn decode_prefix(data: &[u8]) -> Result<(Value, usize)> {
    let mut d = Decoder { data, pos: 0 };
    let v = d.parse_value()?;
    Ok((v, d.pos))
}

/// If `data` is a bencode dict, return the exact raw bytes of the value for
/// `key` (the full bencoded value span). Used to extract `info` bytes for the
/// infohash without re-encoding.
pub fn extract_dict_key_raw<'a>(data: &'a [u8], key: &[u8]) -> Result<Option<&'a [u8]>> {
    if data.is_empty() {
        return Err(Error::UnexpectedEof);
    }
    if data[0] != b'd' {
        return Err(Error::Bencode {
            offset: 0,
            message: "expected dict".into(),
        });
    }
    let mut pos = 1;
    while pos < data.len() {
        if data[pos] == b'e' {
            return Ok(None);
        }
        let (k, k_end) = parse_bytes_at(data, pos)?;
        pos = k_end;
        let value_start = pos;
        pos = skip_value(data, pos)?;
        if k == key {
            return Ok(Some(&data[value_start..pos]));
        }
    }
    Err(Error::UnexpectedEof)
}

struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    fn peek(&self) -> Result<u8> {
        self.data.get(self.pos).copied().ok_or(Error::UnexpectedEof)
    }

    fn bump(&mut self) -> Result<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Ok(b)
    }

    fn parse_value(&mut self) -> Result<Value> {
        match self.peek()? {
            b'i' => self.parse_int(),
            b'l' => self.parse_list(),
            b'd' => self.parse_dict(),
            b'0'..=b'9' => Ok(Value::Bytes(self.parse_bytes()?.to_vec())),
            other => Err(Error::Bencode {
                offset: self.pos,
                message: format!("unexpected byte 0x{other:02x}"),
            }),
        }
    }

    fn parse_int(&mut self) -> Result<Value> {
        let start = self.pos;
        if self.bump()? != b'i' {
            return Err(Error::Bencode {
                offset: start,
                message: "expected 'i'".into(),
            });
        }
        let num_start = self.pos;
        if self.peek()? == b'-' {
            self.pos += 1;
        }
        if self.pos >= self.data.len() {
            return Err(Error::UnexpectedEof);
        }
        // Disallow empty digit runs and leading zeros (except "0").
        let digits_start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos >= self.data.len() || self.data[self.pos] != b'e' {
            return Err(Error::Bencode {
                offset: start,
                message: "unterminated integer".into(),
            });
        }
        let digits = &self.data[digits_start..self.pos];
        if digits.is_empty() {
            return Err(Error::Bencode {
                offset: start,
                message: "empty integer".into(),
            });
        }
        if digits.len() > 1 && digits[0] == b'0' {
            return Err(Error::Bencode {
                offset: start,
                message: "leading zero in integer".into(),
            });
        }
        // "-0" is invalid.
        if self.data[num_start] == b'-' && digits == b"0" {
            return Err(Error::Bencode {
                offset: start,
                message: "negative zero".into(),
            });
        }
        let s =
            std::str::from_utf8(&self.data[num_start..self.pos]).map_err(|_| Error::Bencode {
                offset: start,
                message: "non-utf8 integer".into(),
            })?;
        self.pos += 1; // consume 'e'
        let n: i64 = s.parse().map_err(|_| Error::IntegerOverflow)?;
        Ok(Value::Int(n))
    }

    fn parse_bytes(&mut self) -> Result<&'a [u8]> {
        let start = self.pos;
        let mut len: usize = 0;
        let mut saw_digit = false;
        loop {
            let b = self.bump()?;
            if b == b':' {
                break;
            }
            if !b.is_ascii_digit() {
                return Err(Error::Bencode {
                    offset: start,
                    message: "bad string length".into(),
                });
            }
            if !saw_digit && b == b'0' {
                // Allow "0:" only; reject "01:" etc.
                saw_digit = true;
                let next = self.peek()?;
                if next != b':' {
                    // leading zero with more digits
                    if next.is_ascii_digit() {
                        return Err(Error::Bencode {
                            offset: start,
                            message: "leading zero in string length".into(),
                        });
                    }
                }
                continue;
            }
            saw_digit = true;
            len = len
                .checked_mul(10)
                .and_then(|n| n.checked_add((b - b'0') as usize))
                .ok_or(Error::Bencode {
                    offset: start,
                    message: "string length overflow".into(),
                })?;
        }
        if !saw_digit {
            return Err(Error::Bencode {
                offset: start,
                message: "missing string length".into(),
            });
        }
        let end = self.pos.checked_add(len).ok_or(Error::Bencode {
            offset: start,
            message: "string length overflow".into(),
        })?;
        if end > self.data.len() {
            return Err(Error::UnexpectedEof);
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn parse_list(&mut self) -> Result<Value> {
        let start = self.pos;
        if self.bump()? != b'l' {
            return Err(Error::Bencode {
                offset: start,
                message: "expected 'l'".into(),
            });
        }
        let mut items = Vec::new();
        loop {
            if self.peek()? == b'e' {
                self.pos += 1;
                break;
            }
            items.push(self.parse_value()?);
        }
        Ok(Value::List(items))
    }

    fn parse_dict(&mut self) -> Result<Value> {
        let start = self.pos;
        if self.bump()? != b'd' {
            return Err(Error::Bencode {
                offset: start,
                message: "expected 'd'".into(),
            });
        }
        let mut map = BTreeMap::new();
        loop {
            if self.peek()? == b'e' {
                self.pos += 1;
                break;
            }
            let key = self.parse_bytes()?.to_vec();
            let value = self.parse_value()?;
            // Duplicate keys: last wins (BTreeMap insert). Go is lenient about
            // dict key order on decode; we accept unsorted input.
            map.insert(key, value);
        }
        Ok(Value::Dict(map))
    }
}

fn parse_bytes_at(data: &[u8], pos: usize) -> Result<(&[u8], usize)> {
    let mut d = Decoder { data, pos };
    let b = d.parse_bytes()?;
    Ok((b, d.pos))
}

fn skip_value(data: &[u8], pos: usize) -> Result<usize> {
    let mut d = Decoder { data, pos };
    d.parse_value()?;
    Ok(d.pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_int() {
        assert_eq!(decode(b"i57e").unwrap(), Value::Int(57));
        assert_eq!(
            decode(b"i-9223372036854775808e").unwrap(),
            Value::Int(i64::MIN)
        );
    }

    #[test]
    fn decode_string() {
        assert_eq!(decode(b"5:hello").unwrap(), Value::Bytes(b"hello".to_vec()));
        assert_eq!(decode(b"0:").unwrap(), Value::Bytes(vec![]));
    }

    #[test]
    fn decode_list() {
        let v = decode(b"li5ei10ee").unwrap();
        assert_eq!(v, Value::List(vec![Value::Int(5), Value::Int(10)]));
    }

    #[test]
    fn decode_dict() {
        let v = decode(b"d1:ai5e1:b5:helloe").unwrap();
        let d = v.as_dict().unwrap();
        assert_eq!(d.get(b"a".as_slice()).unwrap(), &Value::Int(5));
        assert_eq!(
            d.get(b"b".as_slice()).unwrap(),
            &Value::Bytes(b"hello".to_vec())
        );
    }

    #[test]
    fn decode_prefix_trailing() {
        let (v, n) = decode_prefix(b"i1ei2e").unwrap();
        assert_eq!(v, Value::Int(1));
        assert_eq!(n, 3);
    }

    #[test]
    fn extract_info_raw() {
        let data = b"d4:infodee";
        let raw = extract_dict_key_raw(data, b"info").unwrap().unwrap();
        assert_eq!(raw, b"de");
    }

    #[test]
    fn reject_trailing() {
        assert!(matches!(
            decode(b"i1ei2e"),
            Err(Error::TrailingBytes { unused: 3 })
        ));
    }

    #[test]
    fn reject_bad() {
        assert!(decode(b"e").is_err());
        assert!(decode(b"i-0e").is_err());
        assert!(decode(b"i01e").is_err());
    }
}
