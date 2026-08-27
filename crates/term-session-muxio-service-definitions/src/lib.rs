#![doc = include_str!("../README.md")]

pub mod channel;
pub mod methods;
pub mod path_wire;

pub use channel::{
    ChannelName, DEFAULT_WORKSPACE, GATEWAY_NAMESPACE, NAMESPACE_ENV_VAR, ProbeOutcome,
    SESSION_CHANNEL_NAME, SESSION_GATEWAY_ENV_VAR, default_generation_hash, gateway_channel_name,
    gateway_help_line, probe_endpoint_outcome, probe_ipc_endpoint,
};
pub use methods::{
    Attach, AttachRequest, ChannelInfo, ClientInfo, CloseSession, KillChannel, KillClient,
    ListChannels, ListChannelsResponse, ListUsers, ListUsersResponse, ListWmStats,
    ListWmStatsResponse, OnAttributedInput, OnAttributedInputRequest, OnPtyResized,
    OnUserConnected, OnUserDisconnected, OnUserResized, OnWorkspaceEntered, OnWorkspaceRebind,
    OnWorkspaceRebindRequest, PushOutput, RPC_ERROR_LIVE_PARTICIPANTS, RPC_ERROR_LIVE_SESSIONS,
    RPC_ERROR_SHUTTING_DOWN, RPC_ERROR_UNATTACHED, RebindScope, RebindWorkspace,
    RebindWorkspaceRequest, ReportWmStats, ResizePty, STREAM_INPUT_METHOD_ID,
    SUBSCRIBE_OUTPUT_METHOD_ID, SendAttributedInput, SendAttributedInputRequest, SessionInfo,
    ShutdownGateway, Spawn, SpawnRequest, SpawnResponse, SubscribeInternalInput,
    SubscribeInternalInputRequest, UserInfo, WmStatsEntry, WriteInput,
};
pub use muxio_rpc_service::prebuffered::RpcMethodPrebuffered;
pub use path_wire::PathWire;
