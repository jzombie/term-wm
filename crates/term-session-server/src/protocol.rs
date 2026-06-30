use bitcode::{Decode, Encode};
use std::io::{self, Read, Write};

pub const MSG_REQUEST: u8 = 0x01;
pub const MSG_RESPONSE: u8 = 0x02;
pub const MSG_PUSH: u8 = 0x03;

#[derive(Encode, Decode, Debug, Clone)]
pub enum SessionServerRequest {
    Spawn {
        cmd: Option<Vec<String>>,
        cols: u16,
        rows: u16,
    },
    Write {
        id: u64,
        data: Vec<u8>,
    },
    Resize {
        id: u64,
        cols: u16,
        rows: u16,
    },
    Close {
        id: u64,
    },
    List,
}

#[derive(Encode, Decode, Debug, Clone)]
pub enum SessionServerResponse {
    Ok { id: Option<u64> },
    Error { msg: String },
    SessionList { sessions: Vec<(u64, String, bool)> },
}

#[derive(Encode, Decode, Debug, Clone)]
pub enum SessionServerPush {
    Welcome { sessions: Vec<(u64, String, bool)> },
    RawOutput { id: u64, data: Vec<u8> },
    Snapshot { id: u64, data: Vec<u8> },
    SessionExited { id: u64, status: i32 },
    TitleChanged { id: u64, title: String },
}

pub fn send_msg(stream: &mut dyn Write, msg_type: u8, payload: &[u8]) -> io::Result<()> {
    let len = payload.len() as u32;
    let header = [
        msg_type,
        len as u8,
        (len >> 8) as u8,
        (len >> 16) as u8,
        (len >> 24) as u8,
    ];
    stream.write_all(&header)?;
    stream.write_all(payload)?;
    stream.flush()
}

pub fn recv_msg(stream: &mut dyn Read) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0u8; 5];
    stream.read_exact(&mut header)?;
    let msg_type = header[0];
    let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]);
    let mut payload = vec![0u8; len as usize];
    if len > 0 {
        stream.read_exact(&mut payload)?;
    }
    Ok((msg_type, payload))
}
