use serde::de::DeserializeOwned;
use serde::Serialize;

use super::error::ProtocolError;
use super::frame::Frame;
use super::version::PROTOCOL_VERSION;

pub fn encode<T: Serialize>(frame: &Frame<T>) -> Result<Vec<u8>, ProtocolError> {
    serde_json::to_vec(frame).map_err(ProtocolError::Serialization)
}

pub fn decode<T: DeserializeOwned>(data: &[u8]) -> Result<Frame<T>, ProtocolError> {
    let frame: Frame<T> = serde_json::from_slice(data).map_err(ProtocolError::Serialization)?;
    if frame.version != PROTOCOL_VERSION {
        return Err(ProtocolError::VersionMismatch(frame.version));
    }
    Ok(frame)
}
