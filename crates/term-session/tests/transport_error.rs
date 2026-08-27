#![allow(clippy::unwrap_used)]

//! Transport error propagation verification.

use std::sync::{Arc, Mutex};

use muxio_core::frame::FrameDecodeError;
use muxio_core::rpc::RpcDispatcher;
use muxio_core::rpc::RpcRequest;
use muxio_core::rpc::rpc_internals::RpcStreamEvent;

/// Direct dispatcher test: `fail_all_pending_requests` with `Transport` must
/// surface the concrete `io::Error` string, not generic `ReadAfterCancel`.
#[test]
fn transport_io_error_propagates_instead_of_generic_cancel() {
    let mut dispatcher = RpcDispatcher::new();
    let called = Arc::new(Mutex::new(None::<String>));
    let called_clone = Arc::clone(&called);

    let req = RpcRequest {
        rpc_method_id: 1,
        rpc_param_bytes: None,
        rpc_prebuffered_payload_bytes: None,
        is_finalized: true,
    };

    let _encoder = dispatcher
        .call(
            req,
            1024,
            |_: &[u8]| {},
            Some(Box::new(move |event| {
                if let RpcStreamEvent::Error {
                    frame_decode_error, ..
                } = event
                {
                    *called_clone.lock().unwrap() = Some(frame_decode_error.to_string());
                }
            })),
            false,
        )
        .expect("call failed");

    // Simulate a real transport reset
    dispatcher
        .fail_all_pending_requests(FrameDecodeError::Transport("ConnectionReset".to_string()));

    let msg = called
        .lock()
        .unwrap()
        .clone()
        .expect("handler should have been called");
    assert!(
        msg.contains("Transport error"),
        "expected Transport error, got: {msg}"
    );
    assert!(
        msg.contains("ConnectionReset"),
        "expected concrete kind, got: {msg}"
    );
    assert!(
        !msg.contains("cancelled stream"),
        "should not be generic ReadAfterCancel, got: {msg}"
    );

    // Clean EOF should synthesize "unexpected EOF"
    let mut dispatcher2 = RpcDispatcher::new();
    let called2 = Arc::new(Mutex::new(None::<String>));
    let called2_clone = Arc::clone(&called2);
    let req2 = RpcRequest {
        rpc_method_id: 2,
        rpc_param_bytes: None,
        rpc_prebuffered_payload_bytes: None,
        is_finalized: true,
    };
    let _encoder2 = dispatcher2
        .call(
            req2,
            1024,
            |_: &[u8]| {},
            Some(Box::new(move |event| {
                if let RpcStreamEvent::Error {
                    frame_decode_error, ..
                } = event
                {
                    *called2_clone.lock().unwrap() = Some(frame_decode_error.to_string());
                }
            })),
            false,
        )
        .expect("call2 failed");
    dispatcher2.fail_all_pending_requests(FrameDecodeError::Transport(
        "unexpected EOF (connection closed)".to_string(),
    ));
    let msg2 = called2.lock().unwrap().clone().unwrap();
    assert!(
        msg2.contains("unexpected EOF"),
        "clean EOF should be synthesized, got: {msg2}"
    );
}

#[test]
fn frame_transport_error_display() {
    use muxio_core::frame::FrameDecodeError;
    let err = FrameDecodeError::Transport("ConnectionReset".to_string());
    assert!(err.to_string().contains("Transport error"));
    assert!(err.to_string().contains("ConnectionReset"));
}
