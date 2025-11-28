mod client;
mod codec;
mod error;
mod frame;
mod server;
mod version;

pub use client::{ClientHello, ClientMessage, CommandRequest, CompletionRequest};
pub use codec::{decode, encode};
pub use error::ProtocolError;
pub use frame::Frame;
pub use server::{
    CommandStatus, ErrorFrame, GoodbyeFrame, OutputFrame, PromptFrame, ServerMessage, ServerWelcome,
};
pub use version::PROTOCOL_VERSION;
