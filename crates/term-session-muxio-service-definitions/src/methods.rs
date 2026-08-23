use std::io;

use bitcode::{Decode, Encode};
use muxio_rpc_service::{prebuffered::RpcMethodPrebuffered, rpc_method_id};

use crate::path_wire::PathWire;
use term_wm_events::Event;

// ── Error message constants ─────────────────────────────────────────
// muxio's wire error only has Fail/System/NotFound codes, so structured
// gateway errors are signalled with well-known message strings that both
// the client and server match on.
pub const RPC_ERROR_UNATTACHED: &str =
    "gateway: connection is not attached to a channel; call Attach first";
pub const RPC_ERROR_SHUTTING_DOWN: &str = "gateway: shutting down";
pub const RPC_ERROR_LIVE_SESSIONS: &str =
    "gateway: live session(s) running; use `--force` to stop anyway";
pub const RPC_ERROR_LIVE_PARTICIPANTS: &str = "gateway: live participant(s) attached to channel; use `term-session kill <channel> --force` to kill anyway";

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
    /// Remote peer source port for SSH attaches; `None` for local attaches.
    pub ssh_port: Option<u16>,
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

/// Request for `Spawn`: join/respawn the session on a channel.
#[derive(Debug, Clone, Encode, Decode)]
pub struct SpawnRequest {
    pub cmd: Option<Vec<String>>,
    pub cols: u16,
    pub rows: u16,
    /// The client process's working directory at the time it launched, so a
    /// newly spawned session starts there rather than in the daemon's cwd.
    /// Encoded losslessly via [`crate::path_wire::encode_path`], so non-UTF-8
    /// paths survive the wire byte-for-byte. The bytes are decoded in the
    /// daemon's host OS context (same host), so the payload is only valid on
    /// the host that produced it. `None`/empty falls back to the daemon's cwd.
    pub cwd: Option<PathWire>,
}

/// Response for `Spawn`: the (possibly reused) session id and its geometry.
#[derive(Debug, Clone, Encode, Decode)]
pub struct SpawnResponse {
    pub id: u64,
    pub cols: u16,
    pub rows: u16,
}

pub struct Spawn;

impl RpcMethodPrebuffered for Spawn {
    const METHOD_ID: u64 = rpc_method_id!("session.spawn");

    type Input = SpawnRequest;
    type Output = SpawnResponse;

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&input))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        bitcode::decode::<SpawnRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn encode_response(output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&output))
    }

    fn decode_response(bytes: &[u8]) -> Result<Self::Output, io::Error> {
        bitcode::decode::<SpawnResponse>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
    pub force: bool,
}

pub struct KillChannel;

impl RpcMethodPrebuffered for KillChannel {
    const METHOD_ID: u64 = rpc_method_id!("session.kill_channel");

    type Input = (String, bool);
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&KillChannelRequest {
            channel: input.0,
            force: input.1,
        }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<KillChannelRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((r.channel, r.force))
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

// ── RebindWorkspace (client asks server to rebind viewers) ───────────

#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq, Default)]
pub enum RebindScope {
    #[default]
    CallerOnly,
    AllViewers,
}

/// Client request to rebind viewers on `source_channel` to `target`.
#[derive(Debug, Clone, Encode, Decode)]
pub struct RebindWorkspaceRequest {
    pub source_channel: String,
    pub target: String,
    pub scope: RebindScope,
    pub initiator_conn_id: Option<usize>,
}

pub struct RebindWorkspace;

impl RpcMethodPrebuffered for RebindWorkspace {
    const METHOD_ID: u64 = rpc_method_id!("session.rebind_workspace");

    type Input = RebindWorkspaceRequest;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&input))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        bitcode::decode::<RebindWorkspaceRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── OnWorkspaceRebind (server pushes to outer viewer) ────────────────

/// Server push telling the outer viewer to rebind to `target`.
#[derive(Debug, Clone, Encode, Decode)]
pub struct OnWorkspaceRebindRequest {
    pub target: String,
}

pub struct OnWorkspaceRebind;

impl RpcMethodPrebuffered for OnWorkspaceRebind {
    const METHOD_ID: u64 = rpc_method_id!("session.on_workspace_rebind");

    type Input = OnWorkspaceRebindRequest;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&input))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        bitcode::decode::<OnWorkspaceRebindRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── SendAttributedInput (client -> server: structured event) ─────────

/// Client sends a structured event to the server for routing.
#[derive(Debug, Clone, Encode, Decode)]
pub struct SendAttributedInputRequest {
    pub channel: String,
    pub event: Event,
}

pub struct SendAttributedInput;

impl RpcMethodPrebuffered for SendAttributedInput {
    const METHOD_ID: u64 = rpc_method_id!("session.send_attributed_input");

    type Input = SendAttributedInputRequest;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&input))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        bitcode::decode::<SendAttributedInputRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── OnAttributedInput (server -> inner WM: attributed event) ─────────

/// Server pushes a structured event with attribution to the inner WM.
#[derive(Debug, Clone, Encode, Decode)]
pub struct OnAttributedInputRequest {
    pub conn_id: usize,
    pub event: Event,
}

pub struct OnAttributedInput;

impl RpcMethodPrebuffered for OnAttributedInput {
    const METHOD_ID: u64 = rpc_method_id!("session.on_attributed_input");

    type Input = OnAttributedInputRequest;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&input))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        bitcode::decode::<OnAttributedInputRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── SubscribeInternalInput (inner WM -> server: register as receiver) ──

/// Inner WM registers itself as the structured input receiver for a channel.
#[derive(Debug, Clone, Encode, Decode)]
pub struct SubscribeInternalInputRequest {
    pub channel: String,
}

pub struct SubscribeInternalInput;

impl RpcMethodPrebuffered for SubscribeInternalInput {
    const METHOD_ID: u64 = rpc_method_id!("session.subscribe_internal_input");

    type Input = SubscribeInternalInputRequest;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&input))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        bitcode::decode::<SubscribeInternalInputRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

// ── WmStats (inner WM -> server: live window/task counts) ───────────

/// One channel's aggregated inner-WM live counts. Windows and tasks are
/// summed across every connected reporter on the channel by the gateway.
#[derive(Debug, Clone, Encode, Decode)]
pub struct WmStatsEntry {
    /// Full channel name (e.g. `dev/main`).
    pub channel: String,
    pub windows: u32,
    pub tasks_running: u32,
}

/// Inner WM reports its live counts (user windows, still-running project
/// tasks). The server resolves the caller's channel from the connection, so
/// the payload carries only the numbers.
pub struct ReportWmStats;

impl RpcMethodPrebuffered for ReportWmStats {
    const METHOD_ID: u64 = rpc_method_id!("session.report_wm_stats");

    type Input = (u32, u32);
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&ReportWmStatsRequestWire {
            windows: input.0,
            tasks_running: input.1,
        }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<ReportWmStatsRequestWire>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((r.windows, r.tasks_running))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

#[derive(Encode, Decode)]
struct ReportWmStatsRequestWire {
    pub windows: u32,
    pub tasks_running: u32,
}

/// Gateway snapshot of every channel's aggregated WM stats. Channels without
/// any reporting connection are omitted (unknown, not zero).
#[derive(Debug, Clone)]
pub struct ListWmStatsResponse {
    pub stats: Vec<WmStatsEntry>,
}

#[derive(Encode, Decode)]
struct ListWmStatsResponseWire {
    pub stats: Vec<WmStatsEntry>,
}

pub struct ListWmStats;

impl RpcMethodPrebuffered for ListWmStats {
    const METHOD_ID: u64 = rpc_method_id!("session.list_wm_stats");

    type Input = ();
    type Output = ListWmStatsResponse;

    fn encode_request(_input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_request(_bytes: &[u8]) -> Result<Self::Input, io::Error> {
        Ok(())
    }

    fn encode_response(output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&ListWmStatsResponseWire {
            stats: output.stats,
        }))
    }

    fn decode_response(bytes: &[u8]) -> Result<Self::Output, io::Error> {
        let r = bitcode::decode::<ListWmStatsResponseWire>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(ListWmStatsResponse { stats: r.stats })
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_request<M: RpcMethodPrebuffered>(input: M::Input) -> M::Input
    where
        M::Input: Clone + Encode + for<'a> Decode<'a>,
    {
        let bytes = M::encode_request(input.clone()).unwrap();
        M::decode_request(&bytes).unwrap()
    }

    #[test]
    fn rebind_workspace_round_trips() {
        let req: RebindWorkspaceRequest = RebindWorkspace::decode_request(
            &RebindWorkspace::encode_request(RebindWorkspaceRequest {
                source_channel: "dev/main".into(),
                target: "ws-123/main".into(),
                scope: RebindScope::CallerOnly,
                initiator_conn_id: Some(42),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(req.source_channel, "dev/main");
        assert_eq!(req.target, "ws-123/main");
        assert_eq!(req.scope, RebindScope::CallerOnly);
        assert_eq!(req.initiator_conn_id, Some(42));
        assert_eq!(RebindWorkspace::decode_response(&[]).unwrap(), ());
    }

    #[test]
    fn on_workspace_rebind_round_trips() {
        let req = roundtrip_request::<OnWorkspaceRebind>(OnWorkspaceRebindRequest {
            target: "ws-123/main".into(),
        });
        assert_eq!(req.target, "ws-123/main");
        assert_eq!(OnWorkspaceRebind::decode_response(&[]).unwrap(), ());
    }

    #[test]
    fn send_attributed_input_round_trips() {
        let req = roundtrip_request::<SendAttributedInput>(SendAttributedInputRequest {
            channel: "dev/main".into(),
            event: Event::Resize(120, 40),
        });
        assert_eq!(req.channel, "dev/main");
        match req.event {
            Event::Resize(cols, rows) => {
                assert_eq!((cols, rows), (120, 40));
            }
            other => panic!("expected Resize event, got {other:?}"),
        }
    }

    #[test]
    fn on_attributed_input_round_trips() {
        let req = roundtrip_request::<OnAttributedInput>(OnAttributedInputRequest {
            conn_id: 42,
            event: Event::FocusGained,
        });
        assert_eq!(req.conn_id, 42);
        assert!(matches!(req.event, Event::FocusGained));
    }

    #[test]
    fn subscribe_internal_input_round_trips() {
        let req = roundtrip_request::<SubscribeInternalInput>(SubscribeInternalInputRequest {
            channel: "dev/main".into(),
        });
        assert_eq!(req.channel, "dev/main");
    }

    #[test]
    fn report_wm_stats_round_trips() {
        let req = roundtrip_request::<ReportWmStats>((7u32, 3u32));
        assert_eq!(req, (7, 3));
        assert_eq!(ReportWmStats::decode_response(&[]).unwrap(), ());
    }

    #[test]
    fn list_wm_stats_round_trips() {
        let out = ListWmStats::decode_response(
            &ListWmStats::encode_response(ListWmStatsResponse {
                stats: vec![
                    WmStatsEntry {
                        channel: "dev/main".into(),
                        windows: 5,
                        tasks_running: 2,
                    },
                    WmStatsEntry {
                        channel: "prod/main".into(),
                        windows: 0,
                        tasks_running: 0,
                    },
                ],
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(out.stats.len(), 2);
        assert_eq!(out.stats[0].channel, "dev/main");
        assert_eq!((out.stats[0].windows, out.stats[0].tasks_running), (5, 2));
        // Empty listing round-trips too.
        let empty = ListWmStats::decode_response(
            &ListWmStats::encode_response(ListWmStatsResponse { stats: vec![] }).unwrap(),
        )
        .unwrap();
        assert!(empty.stats.is_empty());
    }

    #[test]
    fn on_pty_resized_round_trips() {
        let bytes = OnPtyResized::encode_request((200, 60)).unwrap();
        assert_eq!(OnPtyResized::decode_request(&bytes).unwrap(), (200, 60));
    }

    #[test]
    fn malformed_wire_bytes_are_rejected() {
        let bad = b"not-bitcode".to_vec();
        assert!(RebindWorkspace::decode_request(&bad).is_err());
        assert!(OnWorkspaceRebind::decode_request(&bad).is_err());
        assert!(SendAttributedInput::decode_request(&bad).is_err());
        assert!(OnAttributedInput::decode_request(&bad).is_err());
        assert!(SubscribeInternalInput::decode_request(&bad).is_err());
        assert!(OnPtyResized::decode_request(&bad).is_err());
        assert!(OnUserResized::decode_request(&bad).is_err());
    }

    #[test]
    fn error_message_constants_are_stable() {
        assert!(RPC_ERROR_UNATTACHED.starts_with("gateway:"));
        assert!(RPC_ERROR_SHUTTING_DOWN.starts_with("gateway:"));
        assert!(RPC_ERROR_LIVE_SESSIONS.starts_with("gateway:"));
        assert!(RPC_ERROR_LIVE_PARTICIPANTS.starts_with("gateway:"));
    }

    // ── New RPC method roundtrips ──

    #[test]
    fn push_output_round_trips() {
        let input = b"hello world".to_vec();
        let bytes = PushOutput::encode_request(input.clone()).unwrap();
        assert_eq!(PushOutput::decode_request(&bytes).unwrap(), input);
        assert_eq!(
            PushOutput::decode_response(&PushOutput::encode_response(()).unwrap()).unwrap(),
            ()
        );
    }

    #[test]
    fn write_input_round_trips() {
        let input = (42u64, vec![1u8, 2, 3, 4]);
        let bytes = WriteInput::encode_request(input.clone()).unwrap();
        let decoded = WriteInput::decode_request(&bytes).unwrap();
        assert_eq!(decoded.0, 42);
        assert_eq!(decoded.1, vec![1u8, 2, 3, 4]);
    }

    #[test]
    fn list_users_round_trips() {
        let channel = "dev/main".to_string();
        let bytes = ListUsers::encode_request(channel.clone()).unwrap();
        assert_eq!(ListUsers::decode_request(&bytes).unwrap(), channel);

        let response = ListUsersResponse {
            users: vec![
                UserInfo {
                    conn_id: 1,
                    user: "alice".into(),
                    hostname: "host1".into(),
                    ssh_ip: Some("10.0.0.1".into()),
                    ssh_port: Some(2222),
                    cols: 120,
                    rows: 40,
                    connected_at_unix: 1700000000,
                    pid: 12345,
                },
                UserInfo {
                    conn_id: 2,
                    user: "bob".into(),
                    hostname: "host2".into(),
                    ssh_ip: None,
                    ssh_port: None,
                    cols: 80,
                    rows: 24,
                    connected_at_unix: 1700000100,
                    pid: 67890,
                },
            ],
        };
        let resp_bytes = ListUsers::encode_response(response.clone()).unwrap();
        let decoded = ListUsers::decode_response(&resp_bytes).unwrap();
        assert_eq!(decoded.users.len(), 2);
        assert_eq!(decoded.users[0].user, "alice");
        assert_eq!(decoded.users[0].ssh_port, Some(2222));
        assert_eq!(decoded.users[1].user, "bob");
        assert_eq!(decoded.users[1].ssh_port, None);
    }

    #[test]
    fn on_user_connected_round_trips() {
        let info = UserInfo {
            conn_id: 7,
            user: "carol".into(),
            hostname: "laptop".into(),
            ssh_ip: Some("192.168.1.100".into()),
            ssh_port: Some(5555),
            cols: 200,
            rows: 50,
            connected_at_unix: 1700000200,
            pid: 99999,
        };
        let bytes = OnUserConnected::encode_request(info.clone()).unwrap();
        let decoded = OnUserConnected::decode_request(&bytes).unwrap();
        assert_eq!(decoded.conn_id, 7);
        assert_eq!(decoded.user, "carol");
        assert_eq!(decoded.ssh_port, Some(5555));
        assert_eq!(decoded.cols, 200);
        assert_eq!(decoded.pid, 99999);
    }

    #[test]
    fn on_user_disconnected_round_trips() {
        let input = 42usize;
        let bytes = OnUserDisconnected::encode_request(input).unwrap();
        assert_eq!(OnUserDisconnected::decode_request(&bytes).unwrap(), 42);
    }

    #[test]
    fn on_user_resized_round_trips() {
        let bytes = OnUserResized::encode_request((7, 200, 50)).unwrap();
        assert_eq!(OnUserResized::decode_request(&bytes).unwrap(), (7, 200, 50));
    }

    #[test]
    fn on_workspace_entered_round_trips() {
        let input = "ws-123/main".to_string();
        let bytes = OnWorkspaceEntered::encode_request(input.clone()).unwrap();
        assert_eq!(OnWorkspaceEntered::decode_request(&bytes).unwrap(), input);
    }

    #[test]
    fn attach_round_trips() {
        let req = AttachRequest {
            channel: "dev/main".into(),
            hostname: "laptop".into(),
            pid: 12345,
            user: "alice".into(),
            version: "0.1.0".into(),
            ssh_ip: Some("10.0.0.1".into()),
            ssh_port: Some(2222),
        };
        let bytes = Attach::encode_request(req.clone()).unwrap();
        let decoded = Attach::decode_request(&bytes).unwrap();
        assert_eq!(decoded.channel, "dev/main");
        assert_eq!(decoded.hostname, "laptop");
        assert_eq!(decoded.pid, 12345);
        assert_eq!(decoded.user, "alice");
        assert_eq!(decoded.version, "0.1.0");
        assert_eq!(decoded.ssh_ip, Some("10.0.0.1".into()));
        assert_eq!(decoded.ssh_port, Some(2222));

        let resp_bytes = Attach::encode_response(99usize).unwrap();
        assert_eq!(Attach::decode_response(&resp_bytes).unwrap(), 99);
    }

    #[test]
    fn spawn_round_trips() {
        let req = SpawnRequest {
            cmd: Some(vec!["bash".into(), "--login".into()]),
            cols: 120,
            rows: 40,
            cwd: None,
        };
        let bytes = Spawn::encode_request(req).unwrap();
        let decoded = Spawn::decode_request(&bytes).unwrap();
        assert_eq!(decoded.cmd, Some(vec!["bash".into(), "--login".into()]));
        assert_eq!(decoded.cols, 120);
        assert_eq!(decoded.rows, 40);
        assert!(decoded.cwd.is_none());

        let resp = SpawnResponse {
            id: 42,
            cols: 120,
            rows: 40,
        };
        let resp_bytes = Spawn::encode_response(resp).unwrap();
        let decoded_resp = Spawn::decode_response(&resp_bytes).unwrap();
        assert_eq!(decoded_resp.id, 42);
        assert_eq!(decoded_resp.cols, 120);
        assert_eq!(decoded_resp.rows, 40);
    }

    #[test]
    fn resize_pty_round_trips() {
        let input = (5u64, 200u16, 60u16);
        let bytes = ResizePty::encode_request(input).unwrap();
        assert_eq!(ResizePty::decode_request(&bytes).unwrap(), (5, 200, 60));

        let resp = (200u16, 60u16);
        let resp_bytes = ResizePty::encode_response(resp).unwrap();
        assert_eq!(ResizePty::decode_response(&resp_bytes).unwrap(), (200, 60));
    }

    #[test]
    fn close_session_round_trips() {
        let input = 42u64;
        let bytes = CloseSession::encode_request(input).unwrap();
        assert_eq!(CloseSession::decode_request(&bytes).unwrap(), 42);
        assert_eq!(
            CloseSession::decode_response(&CloseSession::encode_response(()).unwrap()).unwrap(),
            ()
        );
    }

    #[test]
    fn write_input_encode_response_is_unit() {
        let resp_bytes = WriteInput::encode_response(()).unwrap();
        assert!(resp_bytes.is_empty());
        assert_eq!(WriteInput::decode_response(&resp_bytes).unwrap(), ());
    }

    #[test]
    fn push_output_encode_response_is_unit() {
        let resp_bytes = PushOutput::encode_response(()).unwrap();
        assert!(resp_bytes.is_empty());
        assert_eq!(PushOutput::decode_response(&resp_bytes).unwrap(), ());
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

// ── UserInfo + presence notifications ─────────────────────────────

/// Presence info for a connected user, shared via `OnUserConnected` and `ListUsers`.
#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
pub struct UserInfo {
    pub conn_id: usize,
    pub user: String,
    pub hostname: String,
    pub ssh_ip: Option<String>,
    /// SSH client source port; `None` for local attaches.
    pub ssh_port: Option<u16>,
    /// Terminal columns reported by the client at spawn.
    pub cols: u16,
    /// Terminal rows reported by the client at spawn.
    pub rows: u16,
    /// Unix timestamp (seconds) when the client connected.
    pub connected_at_unix: u64,
    /// Client process PID (reported at Attach).
    pub pid: u64,
}

/// Server pushes to the active `internal_wm_caller` when a new viewer joins.
pub struct OnUserConnected;

impl RpcMethodPrebuffered for OnUserConnected {
    const METHOD_ID: u64 = rpc_method_id!("session.on_user_connected");

    type Input = UserInfo;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&input))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        bitcode::decode::<UserInfo>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

#[derive(Encode, Decode)]
struct OnUserDisconnectedRequest {
    pub conn_id: usize,
}

/// Server pushes to `internal_wm_caller` when a viewer leaves.
pub struct OnUserDisconnected;

impl RpcMethodPrebuffered for OnUserDisconnected {
    const METHOD_ID: u64 = rpc_method_id!("session.on_user_disconnected");

    type Input = usize;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&OnUserDisconnectedRequest {
            conn_id: input,
        }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<OnUserDisconnectedRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(r.conn_id)
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

#[derive(Encode, Decode)]
struct OnUserResizedRequest {
    pub conn_id: usize,
    pub cols: u16,
    pub rows: u16,
}

/// Server pushes to `internal_wm_caller` when a viewer resizes its terminal.
/// Coalesced server-side (trailing edge) so interactive drag-resizes do not
/// flood the RPC pipeline.
pub struct OnUserResized;

impl RpcMethodPrebuffered for OnUserResized {
    const METHOD_ID: u64 = rpc_method_id!("session.on_user_resized");

    type Input = (usize, u16, u16);
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&OnUserResizedRequest {
            conn_id: input.0,
            cols: input.1,
            rows: input.2,
        }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<OnUserResizedRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((r.conn_id, r.cols, r.rows))
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

#[derive(Encode, Decode)]
struct OnWorkspaceEnteredRequest {
    pub workspace: String,
}

/// Server pushes to `internal_wm_caller` when the viewer lands on a workspace.
pub struct OnWorkspaceEntered;

impl RpcMethodPrebuffered for OnWorkspaceEntered {
    const METHOD_ID: u64 = rpc_method_id!("session.on_workspace_entered");

    type Input = String;
    type Output = ();

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&OnWorkspaceEnteredRequest {
            workspace: input,
        }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<OnWorkspaceEnteredRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(r.workspace)
    }

    fn encode_response(_output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(Vec::new())
    }

    fn decode_response(_bytes: &[u8]) -> Result<Self::Output, io::Error> {
        Ok(())
    }
}

/// Response for `ListUsers`: snapshot of connected users on a channel.
#[derive(Debug, Clone, Encode, Decode)]
pub struct ListUsersResponse {
    pub users: Vec<UserInfo>,
}

#[derive(Encode, Decode)]
struct ListUsersRequest {
    pub channel: String,
}

#[derive(Encode, Decode)]
struct ListUsersResponseWire {
    pub users: Vec<UserInfo>,
}

pub struct ListUsers;

impl RpcMethodPrebuffered for ListUsers {
    const METHOD_ID: u64 = rpc_method_id!("session.list_users");

    type Input = String;
    type Output = ListUsersResponse;

    fn encode_request(input: Self::Input) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&ListUsersRequest { channel: input }))
    }

    fn decode_request(bytes: &[u8]) -> Result<Self::Input, io::Error> {
        let r = bitcode::decode::<ListUsersRequest>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(r.channel)
    }

    fn encode_response(output: Self::Output) -> Result<Vec<u8>, io::Error> {
        Ok(bitcode::encode(&ListUsersResponseWire {
            users: output.users,
        }))
    }

    fn decode_response(bytes: &[u8]) -> Result<Self::Output, io::Error> {
        let r = bitcode::decode::<ListUsersResponseWire>(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(ListUsersResponse { users: r.users })
    }
}
