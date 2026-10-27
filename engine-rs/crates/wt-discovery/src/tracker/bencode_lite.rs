//! Minimal bencode decoder for HTTP tracker announce responses.
//!
//! Temporary until `wt-meta` publishes its bencode API. Only covers the
//! subset needed for announce: dicts, lists, ints, and byte strings.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| std::str::from_utf8(b).ok())
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        match self {
            Self::Dict(d) => Some(d),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Self::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn dict_get<'a>(&'a self, key: &str) -> Option<&'a Value> {
        self.as_dict()?.get(key.as_bytes())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unexpected end of input")]
    Eof,
    #[error("invalid bencode: {0}")]
    Invalid(String),
}

pub fn decode(input: &[u8]) -> Result<(Value, usize), Error> {
    decode_at(input, 0)
}

fn decode_at(input: &[u8], mut i: usize) -> Result<(Value, usize), Error> {
    let b = *input.get(i).ok_or(Error::Eof)?;
    match b {
        b'i' => {
            i += 1;
            let start = i;
            while input.get(i).copied() != Some(b'e') {
                if i >= input.len() {
                    return Err(Error::Eof);
                }
                i += 1;
            }
            let n: i64 = std::str::from_utf8(&input[start..i])
                .map_err(|e| Error::Invalid(e.to_string()))?
                .parse()
                .map_err(|e: std::num::ParseIntError| Error::Invalid(e.to_string()))?;
            Ok((Value::Int(n), i + 1))
        }
        b'l' => {
            i += 1;
            let mut list = Vec::new();
            while input.get(i).copied() != Some(b'e') {
                let (v, ni) = decode_at(input, i)?;
                list.push(v);
                i = ni;
            }
            Ok((Value::List(list), i + 1))
        }
        b'd' => {
            i += 1;
            let mut dict = BTreeMap::new();
            while input.get(i).copied() != Some(b'e') {
                let (k, ni) = decode_at(input, i)?;
                let key = match k {
                    Value::Bytes(b) => b,
                    _ => return Err(Error::Invalid("dict key not string".into())),
                };
                let (v, ni2) = decode_at(input, ni)?;
                dict.insert(key, v);
                i = ni2;
            }
            Ok((Value::Dict(dict), i + 1))
        }
        b'0'..=b'9' => {
            let start = i;
            while input.get(i).copied() != Some(b':') {
                if i >= input.len() {
                    return Err(Error::Eof);
                }
                i += 1;
            }
            let len: usize = std::str::from_utf8(&input[start..i])
                .map_err(|e| Error::Invalid(e.to_string()))?
                .parse()
                .map_err(|e: std::num::ParseIntError| Error::Invalid(e.to_string()))?;
            i += 1;
            let end = i + len;
            if end > input.len() {
                return Err(Error::Eof);
            }
            let bytes = input[i..end].to_vec();
            Ok((Value::Bytes(bytes), end))
        }
        other => Err(Error::Invalid(format!("unexpected byte {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_announce_compact() {
        // d8:intervali1800e5:peers6:\x7f\x00\x00\x01\x1a\xe1e
        let mut body = Vec::new();
        body.extend_from_slice(b"d8:intervali1800e5:peers6:");
        body.extend_from_slice(&[127, 0, 0, 1, 0x1a, 0xe1]);
        body.push(b'e');
        let (v, _) = decode(&body).unwrap();
        assert_eq!(v.dict_get("interval").and_then(|x| x.as_int()), Some(1800));
        assert_eq!(
            v.dict_get("peers")
                .and_then(|x| x.as_bytes())
                .map(|b| b.len()),
            Some(6)
        );
    }
}
