use serde::{Deserialize, Serialize};

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
