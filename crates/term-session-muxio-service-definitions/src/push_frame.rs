use std::io;

/// Tagged frame types multiplexed into the `SessionIo` stream.
///
/// Each frame starts with a one-byte tag, followed by message-type-specific
/// content. The stream is bidirectional:
///   - Client→Server: raw PTY input bytes (keyboard, mouse) — no framing.
///   - Server→Client: tagged frames — RawOutput / SessionExited / TitleChanged.
#[derive(Debug, Clone)]
pub enum SessionPushFrame {
    RawOutput { id: u64, data: Vec<u8> },
    SessionExited { id: u64, status: i32 },
    TitleChanged { id: u64, title: String },
}

// Tag byte constants
const TAG_RAW_OUTPUT: u8 = 0x01;
const TAG_SESSION_EXITED: u8 = 0x02;
const TAG_TITLE_CHANGED: u8 = 0x03;

impl SessionPushFrame {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::RawOutput { id, data } => {
                let id_bytes = id.to_le_bytes();
                let len_bytes = (data.len() as u32).to_le_bytes();
                let mut buf = Vec::with_capacity(1 + 8 + 4 + data.len());
                buf.push(TAG_RAW_OUTPUT);
                buf.extend_from_slice(&id_bytes);
                buf.extend_from_slice(&len_bytes);
                buf.extend_from_slice(data);
                buf
            }
            Self::SessionExited { id, status } => {
                let id_bytes = id.to_le_bytes();
                let status_bytes = status.to_le_bytes();
                let mut buf = Vec::with_capacity(1 + 8 + 4);
                buf.push(TAG_SESSION_EXITED);
                buf.extend_from_slice(&id_bytes);
                buf.extend_from_slice(&status_bytes);
                buf
            }
            Self::TitleChanged { id, title } => {
                let id_bytes = id.to_le_bytes();
                let title_bytes = title.as_bytes();
                let len_bytes = (title_bytes.len() as u32).to_le_bytes();
                let mut buf = Vec::with_capacity(1 + 8 + 4 + title_bytes.len());
                buf.push(TAG_TITLE_CHANGED);
                buf.extend_from_slice(&id_bytes);
                buf.extend_from_slice(&len_bytes);
                buf.extend_from_slice(title_bytes);
                buf
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<(Self, usize), io::Error> {
        if bytes.is_empty() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "empty frame"));
        }
        let tag = bytes[0];
        match tag {
            TAG_RAW_OUTPUT => {
                if bytes.len() < 13 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated raw_output",
                    ));
                }
                let id = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
                let len = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
                if bytes.len() < 13 + len {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated raw_output data",
                    ));
                }
                let data = bytes[13..13 + len].to_vec();
                Ok((Self::RawOutput { id, data }, 13 + len))
            }
            TAG_SESSION_EXITED => {
                if bytes.len() < 13 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated session_exited",
                    ));
                }
                let id = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
                let status = i32::from_le_bytes(bytes[9..13].try_into().unwrap());
                Ok((Self::SessionExited { id, status }, 13))
            }
            TAG_TITLE_CHANGED => {
                if bytes.len() < 13 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated title_changed",
                    ));
                }
                let id = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
                let len = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
                if bytes.len() < 13 + len {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated title_changed data",
                    ));
                }
                let title = String::from_utf8_lossy(&bytes[13..13 + len]).into_owned();
                Ok((Self::TitleChanged { id, title }, 13 + len))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown frame tag: {tag}"),
            )),
        }
    }
}
