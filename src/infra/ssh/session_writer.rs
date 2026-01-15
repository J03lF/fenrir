use std::io::{self, Write};

use russh::server::Session;
use russh::{ChannelId, CryptoVec};

pub struct SessionWriter<'a> {
    session: &'a mut Session,
    channel: ChannelId,
}

impl<'a> SessionWriter<'a> {
    pub fn new(session: &'a mut Session, channel: ChannelId) -> Self {
        Self { session, channel }
    }
}

impl Write for SessionWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut converted: Vec<u8> = Vec::with_capacity(buf.len() * 2);
        for &byte in buf {
            if byte == b'\n' {
                converted.push(b'\r');
                converted.push(b'\n');
            } else {
                converted.push(byte);
            }
        }
        self.session
            .data(self.channel, CryptoVec::from_slice(&converted));
        self.session.flush().map_err(io::Error::other)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.session.flush().map_err(io::Error::other)
    }
}
