use serde::{Deserialize, Serialize};

use super::version::PROTOCOL_VERSION;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Frame<T> {
    pub version: u16,
    pub payload: T,
}

impl<T> Frame<T> {
    pub fn new(payload: T) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            payload,
        }
    }
}
