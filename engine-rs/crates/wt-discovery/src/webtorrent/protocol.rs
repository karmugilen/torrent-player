//! WebTorrent tracker JSON protocol (offers/answers).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Offer {
    pub offer_id: String,
    /// SDP type string ("offer").
    #[serde(rename = "type", default)]
    pub sdp_type: String,
    pub sdp: String,
}

impl Offer {
}

/// Custom serialize for Offer nested under "offer" key as `{type, sdp}` plus offer_id.
#[derive(Clone, Debug, Serialize)]
pub struct AnnounceRequest {
    pub action: String,
    pub info_hash: String,
    pub peer_id: String,
    pub numwant: i32,
    pub uploaded: i64,
    pub downloaded: i64,
    pub left: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub offers: Vec<OfferEnvelope>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OfferEnvelope {
    pub offer_id: String,
    pub offer: SdpBlob,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SdpBlob {
    #[serde(rename = "type")]
    pub sdp_type: String,
    pub sdp: String,
}

impl From<Offer> for OfferEnvelope {
    fn from(o: Offer) -> Self {
        Self {
            offer_id: o.offer_id,
            offer: SdpBlob {
                sdp_type: if o.sdp_type.is_empty() {
                    "offer".into()
                } else {
                    o.sdp_type
                },
                sdp: o.sdp,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct AnnounceResponse {
    #[serde(default)]
    pub info_hash: Option<String>,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub interval: Option<i32>,
    #[serde(default)]
    pub complete: Option<i32>,
    #[serde(default)]
    pub incomplete: Option<i32>,
    #[serde(default)]
    pub peer_id: Option<String>,
    #[serde(default)]
    pub to_peer_id: Option<String>,
    #[serde(default)]
    pub answer: Option<SdpBlob>,
    #[serde(default)]
    pub offer: Option<SdpBlob>,
    #[serde(default)]
    pub offer_id: Option<String>,
}

/// Go `binaryToJsonString`: each byte becomes a Unicode code unit.
pub fn binary_to_json_string(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

pub fn json_string_to_bytes(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let v = c as u32;
        if v > 0xff {
            return Err(format!("rune out of bounds: {v}"));
        }
        out.push(v as u8);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_roundtrip() {
        let b: Vec<u8> = (0u8..=255).collect();
        let s = binary_to_json_string(&b);
        let back = json_string_to_bytes(&s).unwrap();
        assert_eq!(back, b);
    }
}
