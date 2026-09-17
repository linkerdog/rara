use std::collections::BTreeMap;
use std::io::Write;

use rara_app_server::stdio_protocol::{Acknowledgement, ClientFrame, RejectionCode};
use sha2::{Digest, Sha256};

use super::io::Failure;

pub(super) const MAX_REQUESTS: usize = 1024;

struct Receipt {
    fingerprint: [u8; 32],
    acknowledgement: Option<Acknowledgement>,
}

pub(super) enum Admission {
    New,
    Duplicate(Acknowledgement),
    Reject(RejectionCode),
}

#[derive(Default)]
pub(super) struct Receipts(BTreeMap<String, Receipt>);

impl Receipts {
    pub(super) fn admit(&mut self, frame: &ClientFrame) -> Result<Admission, Failure> {
        let mut canonical = serde_json::to_value(frame).map_err(|_| Failure::Framing)?;
        canonical.sort_all_objects();
        let mut writer = Fingerprint(Sha256::new());
        serde_json::to_writer(&mut writer, &canonical).map_err(|_| Failure::Framing)?;
        let fingerprint: [u8; 32] = writer.0.finalize().into();
        if let Some(receipt) = self.0.get(frame.request_id()) {
            if receipt.fingerprint != fingerprint {
                return Ok(Admission::Reject(RejectionCode::RequestConflict));
            }
            return receipt
                .acknowledgement
                .clone()
                .map(Admission::Duplicate)
                .ok_or(Failure::Runtime);
        }
        let limit = if matches!(frame, ClientFrame::Shutdown { .. }) {
            MAX_REQUESTS
        } else {
            MAX_REQUESTS - 1
        };
        if self.0.len() >= limit {
            return Ok(Admission::Reject(RejectionCode::Overloaded));
        }
        self.0.insert(
            frame.request_id().to_owned(),
            Receipt {
                fingerprint,
                acknowledgement: None,
            },
        );
        Ok(Admission::New)
    }

    pub(super) fn finish(&mut self, ack: Acknowledgement) -> Result<(), Failure> {
        let receipt = self.0.get_mut(&ack.request_id).ok_or(Failure::Runtime)?;
        if receipt.acknowledgement.is_some() {
            return Err(Failure::Runtime);
        }
        receipt.acknowledgement = Some(ack);
        Ok(())
    }
}

struct Fingerprint(Sha256);

impl Write for Fingerprint {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
