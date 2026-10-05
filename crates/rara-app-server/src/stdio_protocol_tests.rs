use serde_json::{Value, json};

use super::*;

fn handshake() -> Handshake {
    Handshake {
        protocol_version: 1,
        runtime_version: "test-build".into(),
        runtime_id: "runtime-1".into(),
        transport: "stdio-jsonl".into(),
        request_families: vec!["input".into(), "output".into(), "server".into()],
        request_methods: vec![
            "input.submit_user_prompt".into(),
            "output.replay".into(),
            "server.shutdown".into(),
        ],
        event_families: vec!["assistant".into(), "session".into()],
        capabilities: Capabilities {
            graceful_shutdown: true,
            approval_persistence: false,
            replay: ReplayCapability::Runtime {
                max_events_per_session: 256,
            },
            request_receipts: ReceiptCapability::Runtime { max_requests: 4096 },
        },
        provider: Some("fixture".into()),
        model: None,
    }
}

#[test]
fn handshake_golden_shape_separates_methods_and_lifetimes() {
    let handshake = handshake();
    assert_eq!(handshake.validate(), Ok(()));
    let frame: ServerFrame<Value> = ServerFrame::Handshake(handshake);
    let encoded = encode_server_frame(&frame).unwrap();
    let golden = json!({
        "type": "handshake",
        "payload": {
            "protocol_version": 1,
            "runtime_version": "test-build",
            "runtime_id": "runtime-1",
            "transport": "stdio-jsonl",
            "request_families": ["input", "output", "server"],
            "request_methods": ["input.submit_user_prompt", "output.replay", "server.shutdown"],
            "event_families": ["assistant", "session"],
            "capabilities": {
                "graceful_shutdown": true,
                "approval_persistence": false,
                "replay": {"lifetime": "runtime", "max_events_per_session": 256},
                "request_receipts": {"lifetime": "runtime", "max_requests": 4096}
            },
            "provider": "fixture",
            "model": null
        }
    });
    assert_eq!(serde_json::from_slice::<Value>(&encoded).unwrap(), golden);
    assert_eq!(
        serde_json::from_slice::<ServerFrame<Value>>(&encoded).unwrap(),
        frame
    );
    assert_eq!(encoded.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert_eq!(encoded.last(), Some(&b'\n'));
}

#[test]
fn handshake_rejects_incompatible_versions_and_transport() {
    for version in [0, 2, u32::MAX] {
        let mut h = handshake();
        h.protocol_version = version;
        assert_eq!(h.validate(), Err(ProtocolError::IncompatibleHandshake));
    }
    let mut h = handshake();
    h.transport = "stdio".into();
    assert_eq!(h.validate(), Err(ProtocolError::IncompatibleHandshake));
}

#[test]
fn handshake_requires_nonempty_safe_identity_and_summary_labels() {
    for invalid in ["".into(), " ".into(), "x\ny".into(), "x".repeat(257)] {
        let mut h = handshake();
        h.runtime_version = invalid.clone();
        assert_eq!(h.validate(), Err(ProtocolError::InvalidIdentity));
        h = handshake();
        h.provider = Some(invalid.clone());
        assert_eq!(h.validate(), Err(ProtocolError::InvalidIdentity));
        h = handshake();
        h.model = Some(invalid);
        assert_eq!(h.validate(), Err(ProtocolError::InvalidIdentity));
    }
    let mut h = handshake();
    h.runtime_id = "runtime with spaces".into();
    assert_eq!(h.validate(), Err(ProtocolError::InvalidIdentity));
}

#[test]
fn capabilities_reject_duplicates_bounds_and_family_mismatches() {
    let mut cases = Vec::new();
    let mut h = handshake();
    h.request_methods.push(h.request_methods[0].clone());
    cases.push(h);
    let mut h = handshake();
    h.event_families.clear();
    cases.push(h);
    let mut h = handshake();
    h.event_families = (0..65).map(|index| format!("family-{index}")).collect();
    cases.push(h);
    let mut h = handshake();
    h.event_families.push("invalid family".into());
    cases.push(h);
    let mut h = handshake();
    h.request_families.push("approval".into());
    cases.push(h);
    let mut h = handshake();
    h.request_families.remove(0);
    cases.push(h);
    let mut h = handshake();
    h.request_methods.push("invalid".into());
    cases.push(h);
    let mut h = handshake();
    h.request_methods.push("input.".into());
    cases.push(h);
    for h in cases {
        assert_eq!(h.validate(), Err(ProtocolError::InvalidCapabilities));
    }
}

#[test]
fn shutdown_replay_and_receipt_capabilities_cannot_contradict_methods() {
    let mut cases = Vec::new();
    let mut h = handshake();
    h.capabilities.graceful_shutdown = false;
    cases.push(h);
    let mut h = handshake();
    h.capabilities.replay = ReplayCapability::Unavailable;
    cases.push(h);
    let mut h = handshake();
    h.capabilities.replay = ReplayCapability::Runtime {
        max_events_per_session: 0,
    };
    cases.push(h);
    let mut h = handshake();
    h.capabilities.request_receipts = ReceiptCapability::Runtime { max_requests: 0 };
    cases.push(h);
    for h in cases {
        assert_eq!(h.validate(), Err(ProtocolError::InvalidCapabilities));
    }
    let mut h = handshake();
    h.request_methods.retain(|method| method != "output.replay");
    h.request_families.retain(|family| family != "output");
    h.capabilities.replay = ReplayCapability::Unavailable;
    assert_eq!(h.validate(), Ok(()));
}

#[test]
fn control_golden_shape_preserves_envelope_and_escaped_prompt() {
    let golden = json!({
        "type": "control",
        "payload": {
            "runtime_id": "runtime-1",
            "envelope": {
                "request_id": "request-1",
                "provenance": {
                    "controller": "app_server",
                    "adapter": "supervisor",
                    "session_id": "session-1",
                    "source_id": null,
                    "trust": "untrusted",
                    "authorship": "user_provided"
                },
                "request": {
                    "type": "input",
                    "payload": {"type": "submit_user_prompt", "payload": {"prompt": "first\nsecond"}}
                }
            }
        }
    });
    let frame = decode_client_frame(&serde_json::to_vec(&golden).unwrap()).unwrap();
    assert_eq!(frame.runtime_id(), "runtime-1");
    assert_eq!(frame.request_id(), "request-1");
    assert_eq!(serde_json::to_value(frame).unwrap(), golden);
}

fn control_with_target(request: Value) -> Value {
    json!({
        "type": "control",
        "payload": {
            "runtime_id": "runtime-1",
            "expected_turn_id": "turn-1",
            "envelope": {
                "request_id": "request-1",
                "provenance": {
                    "controller": "app_server", "adapter": "supervisor", "session_id": "session-1",
                    "source_id": null, "trust": "untrusted", "authorship": "user_provided"
                },
                "request": request
            }
        }
    })
}

#[test]
fn stops_and_answers_require_both_session_and_turn_targets() {
    let requests = [
        json!({"type": "session", "payload": {"type": "cancel_current_turn"}}),
        json!({"type": "session", "payload": {"type": "interrupt_current_turn"}}),
        json!({"type": "input", "payload": {"type": "answer_pending_input", "payload": {"answer": "yes"}}}),
        json!({"type": "input", "payload": {"type": "answer_plan_approval", "payload": {"decision": "approve", "feedback": null}}}),
        json!({"type": "input", "payload": {"type": "answer_shell_approval", "payload": {"decision": "once"}}}),
        json!({"type": "approval", "payload": {"type": "answer_pending_approval", "payload": {"approval_id": "approval-1", "approved": true}}}),
    ];
    for request in requests {
        let valid = control_with_target(request);
        let frame = decode_client_frame(&serde_json::to_vec(&valid).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(frame).unwrap(), valid);
        for missing in ["session", "turn", "null_turn"] {
            let mut invalid = valid.clone();
            match missing {
                "session" => {
                    invalid["payload"]["envelope"]["provenance"]
                        .as_object_mut()
                        .unwrap()
                        .remove("session_id");
                }
                "turn" => {
                    invalid["payload"]
                        .as_object_mut()
                        .unwrap()
                        .remove("expected_turn_id");
                }
                "null_turn" => invalid["payload"]["expected_turn_id"] = Value::Null,
                _ => unreachable!(),
            }
            assert_eq!(
                decode_client_frame(&serde_json::to_vec(&invalid).unwrap()),
                Err(ProtocolError::InvalidTarget)
            );
        }
    }
}

#[test]
fn turn_targets_are_rejected_for_unrelated_methods() {
    let requests = [
        json!({"type": "session", "payload": {"type": "create_session"}}),
        json!({"type": "session", "payload": {"type": "resume_session", "payload": {"session_id": "session-1"}}}),
        json!({"type": "session", "payload": {"type": "query_runtime_state"}}),
        json!({"type": "input", "payload": {"type": "submit_user_prompt", "payload": {"prompt": "new"}}}),
        json!({"type": "input", "payload": {"type": "submit_follow_up", "payload": {"prompt": "next"}}}),
        json!({"type": "approval", "payload": {"type": "query_pending_approvals"}}),
    ];
    for request in requests {
        let mut invalid = control_with_target(request);
        assert_eq!(
            decode_client_frame(&serde_json::to_vec(&invalid).unwrap()),
            Err(ProtocolError::InvalidTarget)
        );
        invalid["payload"]
            .as_object_mut()
            .unwrap()
            .remove("expected_turn_id");
        assert!(decode_client_frame(&serde_json::to_vec(&invalid).unwrap()).is_ok());
    }
}

#[test]
fn target_id_bounds_and_codec_errors_do_not_expose_the_reply() {
    let mut frame = control_with_target(
        json!({"type": "input", "payload": {"type": "answer_pending_input", "payload": {"answer": "private-answer"}}}),
    );
    for invalid in [
        "".into(),
        "secret target".into(),
        "x".repeat(MAX_ID_BYTES + 1),
        "\u{2603}".into(),
    ] {
        frame["payload"]["expected_turn_id"] = Value::String(invalid);
        let error = decode_client_frame(&serde_json::to_vec(&frame).unwrap()).unwrap_err();
        assert_eq!(error, ProtocolError::InvalidIdentity);
        assert_eq!(
            error.to_string(),
            "app-server frame has an invalid identity"
        );
    }
    frame["payload"]["expected_turn_id"] = Value::String("x".repeat(MAX_ID_BYTES));
    assert!(decode_client_frame(&serde_json::to_vec(&frame).unwrap()).is_ok());
    frame["payload"]["expected_turn_id"] = Value::Null;
    assert_eq!(
        decode_client_frame(&serde_json::to_vec(&frame).unwrap())
            .unwrap_err()
            .to_string(),
        "app-server control has an invalid turn target"
    );
}

#[test]
fn replay_and_shutdown_golden_shapes_are_correlated() {
    for (golden, expected) in [
        (
            json!({"type":"replay","payload":{"runtime_id":"r","request_id":"q","session_id":"s","after_sequence":7}}),
            ClientFrame::Replay {
                runtime_id: "r".into(),
                request_id: "q".into(),
                session_id: "s".into(),
                after_sequence: 7,
            },
        ),
        (
            json!({"type":"shutdown","payload":{"runtime_id":"r","request_id":"q"}}),
            ClientFrame::Shutdown {
                runtime_id: "r".into(),
                request_id: "q".into(),
            },
        ),
    ] {
        assert_eq!(
            decode_client_frame(&serde_json::to_vec(&golden).unwrap()),
            Ok(expected)
        );
    }
}

#[test]
fn client_frames_reject_missing_identity_unknown_fields_and_invalid_json() {
    for payload in [
        br#"{"type":"shutdown","payload":{"runtime_id":"r","request_id":"q"},"extra":true}"#
            .as_slice(),
        br#"{"type":"shutdown","payload":{"runtime_id":"r","request_id":"q","extra":true}}"#,
        br#"{"type":"shutdown","payload":{"runtime_id":"r"}}"#,
        br#"{"type":"unexpected","payload":{}}"#,
        b"{}\n{}",
        b"\xff",
    ] {
        assert_eq!(
            decode_client_frame(payload),
            Err(ProtocolError::MalformedFrame)
        );
    }
    for id in [
        "".into(),
        "has space".into(),
        "x".repeat(129),
        "\u{2603}".into(),
    ] {
        let payload = json!({"type":"shutdown","payload":{"runtime_id":"r","request_id":id}});
        assert_eq!(
            decode_client_frame(&serde_json::to_vec(&payload).unwrap()),
            Err(ProtocolError::InvalidIdentity)
        );
    }
}

#[test]
fn frame_limit_is_inclusive_and_errors_never_echo_payload() {
    let mut payload =
        br#"{"type":"shutdown","payload":{"runtime_id":"r","request_id":"q"}}"#.to_vec();
    payload.resize(MAX_FRAME_BYTES, b' ');
    assert!(decode_client_frame(&payload).is_ok());
    payload.push(b' ');
    assert_eq!(
        decode_client_frame(&payload),
        Err(ProtocolError::FrameTooLarge)
    );
    let error = decode_client_frame(b"not-json private-credential").unwrap_err();
    assert_eq!(error.to_string(), "app-server frame is malformed");
}

#[test]
fn acknowledgements_do_not_imply_turn_completion() {
    let ack: ServerFrame<Value> = ServerFrame::Ack(Acknowledgement {
        runtime_id: "r".into(),
        request_id: "q".into(),
        result: RequestResult::Accepted {
            session_id: Some("s".into()),
            turn_id: Some("t".into()),
            last_sequence: Some(4),
        },
    });
    assert_eq!(
        serde_json::from_slice::<Value>(&encode_server_frame(&ack).unwrap()).unwrap(),
        json!({
            "type":"ack","payload":{"runtime_id":"r","request_id":"q","result":{"status":"accepted","session_id":"s","turn_id":"t","last_sequence":4}}
        })
    );
    for result in [
        RequestResult::Queued {
            session_id: "s".into(),
        },
        RequestResult::Rejected {
            code: RejectionCode::Busy,
            message: "session is busy".into(),
        },
    ] {
        let ack: ServerFrame<Value> = ServerFrame::Ack(Acknowledgement {
            runtime_id: "r".into(),
            request_id: "q".into(),
            result,
        });
        let encoded = encode_server_frame(&ack).unwrap();
        assert_eq!(
            serde_json::from_slice::<ServerFrame<Value>>(&encoded).unwrap(),
            ack
        );
    }
}

#[test]
fn event_envelope_identifies_the_owned_session_independently_of_provenance() {
    let frame = ServerFrame::Event {
        runtime_id: "runtime-1".into(),
        session_id: "session-1".into(),
        event: json!({"event_id": "ctl-1", "sequence": 1}),
    };
    let encoded = encode_server_frame(&frame).expect("event encoding");
    assert_eq!(
        serde_json::from_slice::<Value>(&encoded).unwrap(),
        json!({"type": "event", "payload": {
            "runtime_id": "runtime-1", "session_id": "session-1",
            "event": {"event_id": "ctl-1", "sequence": 1}
        }})
    );
    assert_eq!(
        serde_json::from_slice::<ServerFrame<Value>>(&encoded).unwrap(),
        frame
    );
}

#[test]
fn output_serialization_stops_at_the_frame_limit() {
    let event = ServerFrame::Event {
        runtime_id: "r".into(),
        session_id: "s".into(),
        event: "x".repeat(MAX_FRAME_BYTES + 1),
    };
    assert_eq!(
        encode_server_frame(&event),
        Err(ProtocolError::FrameTooLarge)
    );
    let mut writer = FrameWriter {
        bytes: Vec::new(),
        exceeded: false,
    };
    writer.write_all(&vec![b'x'; MAX_FRAME_BYTES]).unwrap();
    assert!(writer.write_all(b"x").is_err());
    assert_eq!(writer.bytes.len(), MAX_FRAME_BYTES);
    assert!(writer.bytes.capacity() <= MAX_FRAME_BYTES + 1);
}

#[test]
fn serializer_errors_do_not_expose_private_details() {
    struct PrivateError;
    impl Serialize for PrivateError {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("private-provider-credential"))
        }
    }
    let frame = ServerFrame::Event {
        runtime_id: "r".into(),
        session_id: "s".into(),
        event: PrivateError,
    };
    assert_eq!(
        encode_server_frame(&frame),
        Err(ProtocolError::Serialization)
    );
}
