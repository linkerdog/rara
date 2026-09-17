use std::io::Cursor;

use rara_app_server::stdio_protocol::{ClientFrame, MAX_FRAME_BYTES, RejectionCode, RequestResult};

use super::*;
use crate::app_server_stdio::io::read_frame;
use crate::app_server_stdio::receipts::{Admission, MAX_REQUESTS, Receipts};

fn frame(id: &str) -> ClientFrame {
    ClientFrame::Replay {
        runtime_id: "runtime".into(),
        request_id: id.into(),
        session_id: "session".into(),
        after_sequence: 7,
    }
}

fn acknowledgement(frame: &ClientFrame) -> Acknowledgement {
    Acknowledgement {
        runtime_id: "runtime".into(),
        request_id: frame.request_id().into(),
        result: RequestResult::Accepted {
            session_id: Some("session".into()),
            turn_id: None,
            last_sequence: Some(7),
        },
    }
}

#[test]
fn full_receipts_retain_old_results_and_reserve_shutdown_capacity() {
    let mut receipts = Receipts::default();
    for index in 0..MAX_REQUESTS - 1 {
        let request = frame(&format!("request-{index}"));
        assert!(matches!(
            receipts.admit(&request).expect("admit"),
            Admission::New
        ));
        receipts.finish(acknowledgement(&request)).expect("finish");
    }
    assert!(matches!(
        receipts.admit(&frame("overloaded")).expect("capacity"),
        Admission::Reject(RejectionCode::Overloaded)
    ));
    let original = frame("request-0");
    assert!(
        matches!(receipts.admit(&original).expect("repeat"), Admission::Duplicate(ack) if ack == acknowledgement(&original))
    );
    let mut conflict = original.clone();
    if let ClientFrame::Replay { after_sequence, .. } = &mut conflict {
        *after_sequence = 8;
    }
    assert!(matches!(
        receipts.admit(&conflict).expect("conflict"),
        Admission::Reject(RejectionCode::RequestConflict)
    ));
    let shutdown = ClientFrame::Shutdown {
        runtime_id: "runtime".into(),
        request_id: "shutdown".into(),
    };
    assert!(matches!(
        receipts.admit(&shutdown).expect("reserved shutdown"),
        Admission::New
    ));
    receipts
        .finish(acknowledgement(&shutdown))
        .expect("shutdown receipt");
    assert!(matches!(
        receipts.admit(&original).expect("repeat after full"),
        Admission::Duplicate(_)
    ));
}

#[test]
fn pending_receipts_and_turn_targets_cannot_reapply_a_request() {
    let mut receipts = Receipts::default();
    let request = ClientFrame::Control {
        runtime_id: "runtime".into(),
        expected_turn_id: Some("turn-1".into()),
        envelope: Box::new(RuntimeControlEnvelope {
            request_id: "cancel".into(),
            provenance: RuntimeProvenance::runtime(Some("session".into())),
            request: RuntimeControlRequest::Session(SessionControlRequest::CancelCurrentTurn),
        }),
    };
    assert!(matches!(
        receipts.admit(&request).expect("admit"),
        Admission::New
    ));
    assert!(
        receipts.admit(&request).is_err(),
        "pending is uncertainty, never fresh admission"
    );
    let mut changed = request.clone();
    if let ClientFrame::Control {
        expected_turn_id, ..
    } = &mut changed
    {
        *expected_turn_id = Some("turn-2".into());
    }
    assert!(matches!(
        receipts.admit(&changed).expect("target conflict"),
        Admission::Reject(RejectionCode::RequestConflict)
    ));
    receipts.finish(acknowledgement(&request)).expect("finish");
    assert!(
        receipts.finish(acknowledgement(&request)).is_err(),
        "settlement is immutable"
    );
}

#[test]
fn input_framing_accepts_crlf_and_rejects_truncation_and_oversize() {
    let payload = serde_json::to_vec(&frame("read")).expect("payload");
    for ending in [b"\n".as_slice(), b"\r\n".as_slice()] {
        let mut bytes = payload.clone();
        bytes.extend_from_slice(ending);
        assert_eq!(
            read_frame(&mut Cursor::new(bytes)).expect("frame"),
            Some(frame("read"))
        );
    }
    let mut inclusive = payload.clone();
    inclusive.resize(MAX_FRAME_BYTES, b' ');
    inclusive.extend_from_slice(b"\r\n");
    assert_eq!(
        read_frame(&mut Cursor::new(inclusive)).expect("inclusive CRLF bound"),
        Some(frame("read"))
    );
    assert!(read_frame(&mut Cursor::new(&payload)).is_err());
    assert!(read_frame(&mut Cursor::new(b"\n")).is_err());
    assert!(read_frame(&mut Cursor::new(vec![b'x'; MAX_FRAME_BYTES + 2])).is_err());
    assert!(
        read_frame(&mut Cursor::new(Vec::<u8>::new()))
            .expect("empty EOF")
            .is_none()
    );
}
