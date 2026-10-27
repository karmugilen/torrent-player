//! Bencode encoder.

use super::Value;

/// Encode a `Value` to bencode bytes. Dict keys are written in sorted order
/// (guaranteed by `BTreeMap`).
pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(value, &mut out);
    out
}

fn encode_into(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Int(n) => {
            out.push(b'i');
            out.extend_from_slice(n.to_string().as_bytes());
            out.push(b'e');
        }
        Value::Bytes(b) => {
            out.extend_from_slice(b.len().to_string().as_bytes());
            out.push(b':');
            out.extend_from_slice(b);
        }
        Value::List(items) => {
            out.push(b'l');
            for item in items {
                encode_into(item, out);
            }
            out.push(b'e');
        }
        Value::Dict(map) => {
            out.push(b'd');
            for (k, v) in map {
                out.extend_from_slice(k.len().to_string().as_bytes());
                out.push(b':');
                out.extend_from_slice(k);
                encode_into(v, out);
            }
            out.push(b'e');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::decode;
    use std::collections::BTreeMap;

    #[test]
    fn roundtrip_simple() {
        let cases: &[&[u8]] = &[
            b"i57e",
            b"i-1e",
            b"0:",
            b"5:hello",
            b"le",
            b"li1ei2ee",
            b"de",
            b"d1:ai5e1:b5:helloe",
            b"d0:i420ee",
        ];
        for raw in cases {
            let v = decode(raw).unwrap();
            assert_eq!(encode(&v), *raw, "roundtrip {:?}", std::str::from_utf8(raw));
        }
    }

    #[test]
    fn encode_dict_sorted() {
        let mut m = BTreeMap::new();
        m.insert(b"c".to_vec(), Value::Int(1));
        m.insert(b"a".to_vec(), Value::Int(2));
        assert_eq!(encode(&Value::Dict(m)), b"d1:ai2e1:ci1ee");
    }
}
