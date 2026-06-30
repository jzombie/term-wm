use std::io;

use bitcode::{Decode, Encode};
use muxio_rpc_service::{prebuffered::RpcMethodPrebuffered, rpc_method_id};

// ── Spawn ────────────────────────────────────────────────────────────

#[derive(Encode, Decode)]
struct SpawnRequest {
    pub cmd: Option<Vec<String>>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Encode, Decode)]
struct SpawnResponse {
    pub id: u64,
}

pub struct Spawn;

impl RpcMethodPrebuffered for Spawn {
    const METHOD_ID: u64 = rpc_method_id!("session.spawn");

    type Input = (Option<Vec<String>>, u16, u16);
    type Output = u64;

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&SpawnRequest {
            cmd: input.0,
            cols: input.1,
            rows: input.2,
        }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<SpawnRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((r.cmd, r.cols, r.rows))
    }

    fn encode_response(output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&SpawnResponse { id: output }))
    }

    fn decode_response(bytes: &[u8]) -> Result<Self::Output, io::Error> {
        let r = bitcode::decode::<SpawnResponse>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(r.id)
    }
}

// ── ResizePty ────────────────────────────────────────────────────────

#[derive(Encode, Decode)]
struct ResizeRequest {
    pub id: u64,
    pub cols: u16,
    pub rows: u16,
}

pub struct ResizePty;

impl RpcMethodPrebuffered for ResizePty {
    const METHOD_ID: u64 = rpc_method_id!("session.resize");

    type Input = (u64, u16, u16);
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&ResizeRequest {
            id: input.0,
            cols: input.1,
            rows: input.2,
        }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<ResizeRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((r.id, r.cols, r.rows))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── CloseSession ─────────────────────────────────────────────────────

#[derive(Encode, Decode)]
struct CloseRequest {
    pub id: u64,
}

pub struct CloseSession;

impl RpcMethodPrebuffered for CloseSession {
    const METHOD_ID: u64 = rpc_method_id!("session.close");

    type Input = u64;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&CloseRequest { id: input }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<CloseRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(r.id)
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── WriteInput ───────────────────────────────────────────────────────

#[derive(Encode, Decode)]
struct WriteInputRequest {
    pub id: u64,
    pub data: Vec<u8>,
}

pub struct WriteInput;

impl RpcMethodPrebuffered for WriteInput {
    const METHOD_ID: u64 = rpc_method_id!("session.write_input");

    type Input = (u64, Vec<u8>);
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&WriteInputRequest {
            id: input.0,
            data: input.1,
        }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<WriteInputRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((r.id, r.data))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── PushOutput ───────────────────────────────────────────────────────

pub struct PushOutput;

impl RpcMethodPrebuffered for PushOutput {
    const METHOD_ID: u64 = rpc_method_id!("session.push_output");

    type Input = Vec<u8>;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(input)
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        Ok(bytes.to_vec())
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── StreamInput (streaming handler) ─────────────────────────────────
pub const STREAM_INPUT_METHOD_ID: u64 = rpc_method_id!("session.stream_input");

// ── ListSessions ─────────────────────────────────────────────────────

#[derive(Encode, Decode)]
struct ListResponse {
    pub sessions: Vec<(u64, String, bool)>,
}

pub struct ListSessions;

impl RpcMethodPrebuffered for ListSessions {
    const METHOD_ID: u64 = rpc_method_id!("session.list");

    type Input = ();
    type Output = Vec<(u64, String, bool)>;

    fn encode_request(_input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_request(_bytes: &[u8]) -> Result<Self::Input, io::Error> {
        Ok(())
    }

    fn encode_response(output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&ListResponse { sessions: output }))
    }

    fn decode_response(bytes: &[u8]) -> Result<Self::Output, io::Error> {
        let r = bitcode::decode::<ListResponse>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(r.sessions)
    }
}
