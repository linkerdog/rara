//! Versioned subprocess frames, independent of runtime implementation ownership.

use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, Write};

use serde::{Deserialize, Serialize};

use crate::runtime_control::RuntimeControlEnvelope;

pub const PROTOCOL_VERSION: u32 = 1;
pub const TRANSPORT: &str = "stdio-jsonl";
pub const MAX_FRAME_BYTES: usize = 1_048_576;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_CAPABILITY_ENTRIES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handshake {
    pub protocol_version: u32,
    pub runtime_version: String,
    pub runtime_id: String,
    pub transport: String,
    pub request_families: Vec<String>,
    pub request_methods: Vec<String>,
    pub event_families: Vec<String>,
    pub capabilities: Capabilities,
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub graceful_shutdown: bool,
    pub approval_persistence: bool,
    pub replay: ReplayCapability,
    pub request_receipts: ReceiptCapability,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "lifetime", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplayCapability {
    Unavailable,
    Runtime { max_events_per_session: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "lifetime", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiptCapability {
    Runtime { max_requests: u32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ClientFrame {
    Control {
        runtime_id: String,
        envelope: Box<RuntimeControlEnvelope>,
    },
    Replay {
        runtime_id: String,
        request_id: String,
        session_id: String,
        after_sequence: u64,
    },
    Shutdown {
        runtime_id: String,
        request_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ServerFrame<Event> {
    Handshake(Handshake),
    Ack(Acknowledgement),
    Event {
        runtime_id: String,
        event: Event,
    },
    ReplayGap(ReplayGap),
    ShutdownComplete {
        runtime_id: String,
        request_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acknowledgement {
    pub runtime_id: String,
    pub request_id: String,
    pub result: RequestResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequestResult {
    Accepted {
        session_id: Option<String>,
        turn_id: Option<String>,
        last_sequence: Option<u64>,
    },
    Queued {
        session_id: String,
    },
    Rejected {
        code: RejectionCode,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionCode {
    InvalidRequest,
    StaleRuntime,
    UnknownSession,
    Unsupported,
    Busy,
    NotRunning,
    Overloaded,
    Closed,
    RequestConflict,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayGap {
    pub runtime_id: String,
    pub request_id: String,
    pub session_id: String,
    pub requested_after: u64,
    pub oldest_available: u64,
    pub latest: u64,
}

/// Safe codec categories must never contain raw frames or provider diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    FrameTooLarge,
    MalformedFrame,
    InvalidIdentity,
    InvalidCapabilities,
    IncompatibleHandshake,
    Serialization,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::FrameTooLarge => "app-server frame exceeds the byte limit",
            Self::MalformedFrame => "app-server frame is malformed",
            Self::InvalidIdentity => "app-server frame has an invalid identity",
            Self::InvalidCapabilities => "app-server capabilities are invalid",
            Self::IncompatibleHandshake => "app-server handshake is incompatible",
            Self::Serialization => "app-server frame serialization failed",
        })
    }
}

impl std::error::Error for ProtocolError {}

impl Handshake {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.protocol_version != PROTOCOL_VERSION || self.transport != TRANSPORT {
            return Err(ProtocolError::IncompatibleHandshake);
        }
        validate_id(&self.runtime_id)?;
        for label in [
            Some(&self.runtime_version),
            self.provider.as_ref(),
            self.model.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if label.trim().is_empty() || label.len() > 256 || label.chars().any(char::is_control) {
                return Err(ProtocolError::InvalidIdentity);
            }
        }
        for entries in [
            &self.request_families,
            &self.request_methods,
            &self.event_families,
        ] {
            if entries.is_empty()
                || entries.len() > MAX_CAPABILITY_ENTRIES
                || entries.iter().collect::<BTreeSet<_>>().len() != entries.len()
            {
                return Err(ProtocolError::InvalidCapabilities);
            }
            for entry in entries {
                validate_id(entry).map_err(|_| ProtocolError::InvalidCapabilities)?;
            }
        }
        let mut method_families = BTreeSet::new();
        for method in &self.request_methods {
            let Some((family, operation)) = method.split_once('.') else {
                return Err(ProtocolError::InvalidCapabilities);
            };
            if family.is_empty() || operation.is_empty() {
                return Err(ProtocolError::InvalidCapabilities);
            }
            method_families.insert(family);
        }
        if method_families != self.request_families.iter().map(String::as_str).collect() {
            return Err(ProtocolError::InvalidCapabilities);
        }
        if self.capabilities.graceful_shutdown
            != self
                .request_methods
                .iter()
                .any(|method| method == "server.shutdown")
        {
            return Err(ProtocolError::InvalidCapabilities);
        }
        match self.capabilities.replay {
            ReplayCapability::Unavailable => {
                if self
                    .request_methods
                    .iter()
                    .any(|method| method == "output.replay")
                {
                    return Err(ProtocolError::InvalidCapabilities);
                }
            }
            ReplayCapability::Runtime {
                max_events_per_session,
            } => {
                if max_events_per_session == 0
                    || !self
                        .request_methods
                        .iter()
                        .any(|method| method == "output.replay")
                {
                    return Err(ProtocolError::InvalidCapabilities);
                }
            }
        }
        match self.capabilities.request_receipts {
            ReceiptCapability::Runtime { max_requests: 0 } => {
                return Err(ProtocolError::InvalidCapabilities);
            }
            ReceiptCapability::Runtime { .. } => {}
        }
        Ok(())
    }
}

impl ClientFrame {
    pub fn runtime_id(&self) -> &str {
        match self {
            Self::Control { runtime_id, .. }
            | Self::Replay { runtime_id, .. }
            | Self::Shutdown { runtime_id, .. } => runtime_id,
        }
    }

    pub fn request_id(&self) -> &str {
        match self {
            Self::Control { envelope, .. } => &envelope.request_id,
            Self::Replay { request_id, .. } | Self::Shutdown { request_id, .. } => request_id,
        }
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_id(self.runtime_id())?;
        validate_id(self.request_id())?;
        let session_id = match self {
            Self::Control { envelope, .. } => envelope.provenance.session_id.as_deref(),
            Self::Replay { session_id, .. } => Some(session_id.as_str()),
            Self::Shutdown { .. } => None,
        };
        if let Some(session_id) = session_id {
            validate_id(session_id)?;
        }
        Ok(())
    }
}

/// Decode one already-delimited payload; the caller must bound its input buffer.
pub fn decode_client_frame(payload: &[u8]) -> Result<ClientFrame, ProtocolError> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    if payload.contains(&b'\n') || payload.contains(&b'\r') {
        return Err(ProtocolError::MalformedFrame);
    }
    let frame: ClientFrame =
        serde_json::from_slice(payload).map_err(|_| ProtocolError::MalformedFrame)?;
    frame.validate()?;
    Ok(frame)
}

/// Serialize a single output frame without allocating beyond its wire limit.
pub fn encode_server_frame<Event: Serialize>(
    frame: &ServerFrame<Event>,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = FrameWriter {
        bytes: Vec::new(),
        exceeded: false,
    };
    if serde_json::to_writer(&mut writer, frame).is_err() {
        return Err(if writer.exceeded {
            ProtocolError::FrameTooLarge
        } else {
            ProtocolError::Serialization
        });
    }
    writer.bytes.reserve_exact(1);
    writer.bytes.push(b'\n');
    Ok(writer.bytes)
}

fn validate_id(id: &str) -> Result<(), ProtocolError> {
    if id.is_empty()
        || id.len() > MAX_ID_BYTES
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
    {
        return Err(ProtocolError::InvalidIdentity);
    }
    Ok(())
}

struct FrameWriter {
    bytes: Vec<u8>,
    exceeded: bool,
}

impl Write for FrameWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_FRAME_BYTES.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("app-server frame exceeds the byte limit"));
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            let capacity = required.next_power_of_two().min(MAX_FRAME_BYTES + 1);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "stdio_protocol_tests.rs"]
mod tests;
