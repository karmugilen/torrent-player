//! BitTorrent wire protocol primitives re-exported from `wt-wire`.

pub use wt_wire::codec::{decode_message, DEFAULT_MAX_MESSAGE_LENGTH};
pub use wt_wire::ext::{
    ExtendedHandshake, MetadataMsg, MetadataMsgType, PexMsg, EXTENSION_NAME_METADATA,
    EXTENSION_NAME_PEX, METADATA_PIECE_SIZE,
};
pub use wt_wire::handshake::{
    default_peer_extension_bytes, handshake, handshake_incoming, read_handshake, write_handshake,
    HandshakeResult, PeerExtensionBits, EXTENSION_BIT_FAST, EXTENSION_BIT_LTEP, HANDSHAKE_LEN,
    PROTOCOL_HEADER, PROTOCOL_STR,
};
pub use wt_wire::message::{marshal_bitfield, unmarshal_bitfield, Message, MessageType};

pub const EXT_HANDSHAKE_ID: u8 = 0;
pub const DEFAULT_UT_METADATA_ID: u8 = 1;
pub const DEFAULT_UT_PEX_ID: u8 = 2;
