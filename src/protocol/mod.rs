use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;

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

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello(ClientHello),
    Command(CommandRequest),
    Complete(CompletionRequest),
    Exit,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClientHello {
    pub client_id: String,
    pub hostname: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommandRequest {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CompletionRequest {
    pub line: String,
    pub cursor: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome(ServerWelcome),
    Prompt(PromptFrame),
    Output(OutputFrame),
    Error(ErrorFrame),
    Goodbye(GoodbyeFrame),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServerWelcome {
    pub banner: String,
    pub motd: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PromptFrame {
    pub prompt: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OutputFrame {
    pub status: CommandStatus,
    pub lines: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Continue,
    EnterSubshell,
    Exit,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ErrorFrame {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GoodbyeFrame {
    pub reason: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum ProtocolError {
    #[error("serialisierung fehlgeschlagen: {0}")]
    Serialization(serde_json::Error),
    #[error("veraltete protokollversion: {0}")]
    VersionMismatch(u16),
}

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
