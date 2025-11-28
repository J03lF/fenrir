use serde::{Deserialize, Serialize};

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
