#![forbid(unsafe_code)]

pub use wt_meta::bencode;
pub mod codec;
pub mod ext;
pub mod handshake;
pub mod message;
pub mod mse;
pub mod transport;
pub mod utp;

// Re-export common types
pub use codec::{
    decode_message, FramedReader, FramedWriter, MsgWriterStats, PeerConnMsgWriter,
    DEFAULT_MAX_MESSAGE_LENGTH, WRITE_BUFFER_HIGH_WATER_LEN,
};
pub use ext::{
    ExtendedHandshake, LocalLtepProtocolMap, MetadataMsg, MetadataMsgType, PexMsg,
    EXTENSION_NAME_METADATA, EXTENSION_NAME_PEX, METADATA_PIECE_SIZE,
};
pub use handshake::{
    default_peer_extension_bytes, handshake, handshake_incoming, read_handshake, write_handshake,
    HandshakeResult, PeerExtensionBits, EXTENSION_BIT_DHT, EXTENSION_BIT_FAST, EXTENSION_BIT_LTEP,
    EXTENSION_BIT_V2_UPGRADE, HANDSHAKE_LEN, PROTOCOL_HEADER, PROTOCOL_STR,
};
pub use message::{marshal_bitfield, unmarshal_bitfield, Message, MessageType};
pub use mse::{
    default_crypto_selector, handle_encryption, initiate_handshake, receive_handshake,
    CryptoMethod, HeaderObfuscationPolicy, MseStream, ReceiverResult,
};
pub use transport::{
    dial_peer, dial_tcp, dial_utp, listen_dual_stack_tcp, listen_dual_stack_utp, listen_tcp,
    listen_utp, WireStream,
};
pub use utp::{split_extensions, PacketHeader, UtpSocket, UtpStream};
