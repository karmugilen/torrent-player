//! Property-based round-trip tests for bencode Values.

use quickcheck::{Arbitrary, Gen, QuickCheck};
use std::collections::BTreeMap;
use wt_meta::bencode::{decode, encode, Value};

#[derive(Clone, Debug)]
struct ArbValue(Value);

impl Arbitrary for ArbValue {
    fn arbitrary(g: &mut Gen) -> Self {
        ArbValue(arb_value(g, 0))
    }
}

fn arb_value(g: &mut Gen, depth: usize) -> Value {
    if depth > 3 {
        return arb_leaf(g);
    }
    match u8::arbitrary(g) % 4 {
        0 => Value::Int(i64::arbitrary(g)),
        1 => Value::Bytes(Vec::<u8>::arbitrary(g)),
        2 => {
            let n = usize::arbitrary(g) % 6;
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                items.push(arb_value(g, depth + 1));
            }
            Value::List(items)
        }
        _ => {
            let n = usize::arbitrary(g) % 5;
            let mut map = BTreeMap::new();
            for _ in 0..n {
                let key = Vec::<u8>::arbitrary(g);
                // Cap key size for speed.
                let key = if key.len() > 32 {
                    key[..32].to_vec()
                } else {
                    key
                };
                map.insert(key, arb_value(g, depth + 1));
            }
            Value::Dict(map)
        }
    }
}

fn arb_leaf(g: &mut Gen) -> Value {
    if bool::arbitrary(g) {
        Value::Int(i64::arbitrary(g))
    } else {
        let mut b = Vec::<u8>::arbitrary(g);
        if b.len() > 64 {
            b.truncate(64);
        }
        Value::Bytes(b)
    }
}

#[test]
fn bencode_roundtrip_quickcheck() {
    fn prop(ArbValue(v): ArbValue) -> bool {
        let encoded = encode(&v);
        match decode(&encoded) {
            Ok(decoded) => decoded == v,
            Err(_) => false,
        }
    }
    QuickCheck::new()
        .tests(256)
        .max_tests(512)
        .quickcheck(prop as fn(ArbValue) -> bool);
}

#[test]
fn bencode_decode_never_panics_on_random_bytes() {
    fn prop(data: Vec<u8>) -> bool {
        let _ = decode(&data);
        let _ = wt_meta::bencode::decode_prefix(&data);
        true
    }
    QuickCheck::new()
        .tests(256)
        .quickcheck(prop as fn(Vec<u8>) -> bool);
}
