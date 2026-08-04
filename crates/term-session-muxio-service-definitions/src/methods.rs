use std::io;

use bitcode::{Decode, Encode};
use muxio_rpc_service::{prebuffered::RpcMethodPrebuffered, rpc_method_id};

// ── Error message constants ─────────────────────────────────────────
// muxio's wire error only has Fail/System/NotFound codes, so structured
// gateway errors are signalled with well-known message strings that both
// the client and server match on.
pub const RPC_ERROR_UNATTACHED: &str =
    "gateway: connection is not attached to a channel; call Attach first";
pub const RPC_ERROR_SHUTTING_DOWN: &str = "gateway: shutting down";
pub const RPC_ERROR_LIVE_SESSIONS: &str =
    "gateway: live session(s) running; use `term-session stop --force` to stop anyway";

// ── Attach ──────────────────────────────────────────────────────────

/// Client-provided identity reported at `Attach` so `list` can show who each
/// socket belongs to and where it came from.
#[derive(Debug, Clone, Encode, Decode)]
pub struct AttachRequest {
    pub channel: String,
    pub hostname: String,
    /// The client process's OS PID, so `list` can show which PID is attached.
    pub pid: u64,
    /// OS user running the client process (e.g. `jzombie`).
    pub user: String,
    /// Client binary version (`CARGO_PKG_VERSION`).
    pub version: String,
    /// Remote peer IP for SSH attaches; `None` for local attaches.
    pub ssh_ip: Option<String>,
}

#[derive(Encode, Decode)]
struct AttachResponse {
    pub conn_id: usize,
}

pub struct Attach;

impl RpcMethodPrebuffered for Attach {
    const METHOD_ID: u64 = rpc_method_id!("session.attach");

    type Input = AttachRequest;
    type Output = usize;

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&input))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        bitcode::decode::<AttachRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn encode_response(output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&AttachResponse { conn_id: output }))
    }

    fn decode_response(bytes: &[u8]) -> Result<Self::Output, io::Error> {
        let r = bitcode::decode::<AttachResponse>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(r.conn_id)
    }
}

// ── Spawn ────────────────────────────────────────────────────────────

#[derive(Encode, Decode)]
struct SpawnRequest {
    pub cmd: Option<Vec<String>>,
    pub cols: u16,
    pub rows: u16,
    /// The client process's working directory at the time it launched, so a
    /// newly spawned session starts there rather than in the daemon's cwd.
    /// `None`/empty falls back to the daemon's cwd.
    pub cwd: Option<String>,
}

#[derive(Encode, Decode)]
struct SpawnResponse {
    pub id: u64,
    pub cols: u16,
    pub rows: u16,
}

pub struct Spawn;

impl RpcMethodPrebuffered for Spawn {
    const METHOD_ID: u64 = rpc_method_id!("session.spawn");

    type Input = (Option<Vec<String>>, u16, u16, Option<String>);
    type Output = (u64, u16, u16);

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&SpawnRequest {
            cmd: input.0,
            cols: input.1,
            rows: input.2,
            cwd: input.3,
        }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<SpawnRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((r.cmd, r.cols, r.rows, r.cwd))
    }

    fn encode_response(output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&SpawnResponse {
            id: output.0,
            cols: output.1,
            rows: output.2,
        }))
    }

    fn decode_response(bytes: &[u8]) -> Result<Self::Output, io::Error> {
        let r = bitcode::decode::<SpawnResponse>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((r.id, r.cols, r.rows))
    }
}

// ── ResizePty ────────────────────────────────────────────────────────

#[derive(Encode, Decode)]
struct ResizeRequest {
    pub id: u64,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Encode, Decode)]
struct ResizeResponse {
    pub cols: u16,
    pub rows: u16,
}

pub struct ResizePty;

impl RpcMethodPrebuffered for ResizePty {
    const METHOD_ID: u64 = rpc_method_id!("session.resize");

    type Input = (u64, u16, u16);
    type Output = (u16, u16);

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

    fn encode_response(output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&ResizeResponse {
            cols: output.0,
            rows: output.1,
        }))
    }

    fn decode_response(bytes: &[u8]) -> Result<Self::Output, io::Error> {
        let r = bitcode::decode::<ResizeResponse>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((r.cols, r.rows))
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

// ── StreamInput (streaming handler for PTY input) ──────────────────
pub const STREAM_INPUT_METHOD_ID: u64 = rpc_method_id!("session.stream_input");

// ── SubscribeOutput (streaming handler for PTY output pushes) ──────
pub const SUBSCRIBE_OUTPUT_METHOD_ID: u64 = rpc_method_id!("session.subscribe_output");

// ── ListChannels ─────────────────────────────────────────────────────

/// Public wire info for one session on a channel.
#[derive(Debug, Clone, Encode, Decode)]
pub struct SessionInfo {
    pub id: u64,
    pub cols: u16,
    pub rows: u16,
    pub exited: bool,
    pub exit_code: Option<i32>,
    pub title: String,
}

/// Public wire info for one attached client socket on a channel.
#[derive(Debug, Clone, Encode, Decode)]
pub struct ClientInfo {
    pub conn_id: usize,
    /// The client process's OS PID (reported at Attach).
    pub pid: u64,
    pub hostname: String,
    pub connected_at_unix: u64,
    pub cols: u16,
    pub rows: u16,
    /// OS user running the client process (reported at Attach).
    pub user: String,
    /// Client binary version (reported at Attach).
    pub version: String,
    /// Remote peer IP for SSH attaches; `None` for local attaches.
    pub ssh_ip: Option<String>,
}

/// Public wire info for one channel on the gateway.
#[derive(Debug, Clone, Encode, Decode)]
pub struct ChannelInfo {
    pub name: String,
    pub created_at_unix: u64,
    pub session: Option<SessionInfo>,
    pub clients: Vec<ClientInfo>,
}

/// Response for `ListChannels`: the gateway's PID + bound socket name plus the
/// full channel listing. The PID lets the CLI identify the daemon process
/// unambiguously in process managers.
#[derive(Debug, Clone, Encode, Decode)]
pub struct ListChannelsResponse {
    pub gateway_pid: u64,
    pub socket: String,
    pub channels: Vec<ChannelInfo>,
}

#[derive(Encode, Decode)]
struct ListChannelsResponseWire {
    pub gateway_pid: u64,
    pub socket: String,
    pub channels: Vec<ChannelInfo>,
}

pub struct ListChannels;

impl RpcMethodPrebuffered for ListChannels {
    const METHOD_ID: u64 = rpc_method_id!("session.list_channels");

    type Input = ();
    type Output = ListChannelsResponse;

    fn encode_request(_input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_request(_bytes: &[u8]) -> Result<Self::Input, io::Error> {
        Ok(())
    }

    fn encode_response(output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&ListChannelsResponseWire {
            gateway_pid: output.gateway_pid,
            socket: output.socket,
            channels: output.channels,
        }))
    }

    fn decode_response(bytes: &[u8]) -> Result<Self::Output, io::Error> {
        let r = bitcode::decode::<ListChannelsResponseWire>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(ListChannelsResponse {
            gateway_pid: r.gateway_pid,
            socket: r.socket,
            channels: r.channels,
        })
    }
}

// ── KillChannel ──────────────────────────────────────────────────────

#[derive(Encode, Decode)]
struct KillChannelRequest {
    pub channel: String,
}

pub struct KillChannel;

impl RpcMethodPrebuffered for KillChannel {
    const METHOD_ID: u64 = rpc_method_id!("session.kill_channel");

    type Input = String;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&KillChannelRequest { channel: input }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<KillChannelRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(r.channel)
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── KillClient ───────────────────────────────────────────────────────

#[derive(Encode, Decode)]
struct KillClientRequest {
    pub channel: String,
    pub conn_id: usize,
}

pub struct KillClient;

impl RpcMethodPrebuffered for KillClient {
    const METHOD_ID: u64 = rpc_method_id!("session.kill_client");

    type Input = (String, usize);
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&KillClientRequest {
            channel: input.0,
            conn_id: input.1,
        }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<KillClientRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((r.channel, r.conn_id))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── ShutdownGateway ──────────────────────────────────────────────────

#[derive(Encode, Decode)]
struct ShutdownGatewayRequest {
    pub force: bool,
}

pub struct ShutdownGateway;

impl RpcMethodPrebuffered for ShutdownGateway {
    const METHOD_ID: u64 = rpc_method_id!("session.shutdown_gateway");

    type Input = bool;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&ShutdownGatewayRequest { force: input }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<ShutdownGatewayRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(r.force)
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── OnPtyResized (server calls client to notify geometry change) ─────

#[derive(Encode, Decode)]
struct OnPtyResizedRequest {
    pub cols: u16,
    pub rows: u16,
}

pub struct OnPtyResized;

impl RpcMethodPrebuffered for OnPtyResized {
    const METHOD_ID: u64 = rpc_method_id!("session.on_pty_resized");

    type Input = (u16, u16);
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&OnPtyResizedRequest {
            cols: input.0,
            rows: input.1,
        }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<OnPtyResizedRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((r.cols, r.rows))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}
