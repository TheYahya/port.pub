use serde::{Deserialize, Serialize};
use tokio_util::codec::AnyDelimiterCodec;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello,
    Accept(Uuid),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ServerMessage {
    SubDomain(String),
    Connection(Uuid),
}

pub fn new_codec() -> AnyDelimiterCodec {
    AnyDelimiterCodec::new_with_max_length(vec![0], vec![0], 128)
}
