//! Bencode encode/decode (BEP 3).
//!
//! `Value` is the decoded tree. Dict keys are raw bytes ordered by `BTreeMap`
//! (lexicographic), matching the bencode wire requirement.

mod decode;
mod encode;

pub use decode::{decode, decode_prefix, extract_dict_key_raw};
pub use encode::encode;

use std::collections::BTreeMap;
use std::fmt;

/// A bencode value.
#[derive(Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    /// Ordered by key bytes (bencode dicts must be sorted).
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "Int({n})"),
            Value::Bytes(b) => {
                if let Ok(s) = std::str::from_utf8(b) {
                    write!(f, "Bytes({s:?})")
                } else {
                    write!(f, "Bytes({b:?})")
                }
            }
            Value::List(l) => f.debug_tuple("List").field(l).finish(),
            Value::Dict(d) => {
                let mut m = f.debug_map();
                for (k, v) in d {
                    if let Ok(s) = std::str::from_utf8(k) {
                        m.entry(&s, v);
                    } else {
                        m.entry(k, v);
                    }
                }
                m.finish()
            }
        }
    }
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| std::str::from_utf8(b).ok())
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn dict_get(&self, key: &[u8]) -> Option<&Value> {
        self.as_dict()?.get(key)
    }

    pub fn dict_get_str(&self, key: &str) -> Option<&Value> {
        self.dict_get(key.as_bytes())
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Int(n)
    }
}

impl From<Vec<u8>> for Value {
    fn from(b: Vec<u8>) -> Self {
        Value::Bytes(b)
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Bytes(s.into_bytes())
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Bytes(s.as_bytes().to_vec())
    }
}
