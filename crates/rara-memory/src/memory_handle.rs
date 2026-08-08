/// Session-scoped local memory handle.
///
/// Short-term memory now uses local files. Durable recall and semantic
/// retrieval are delegated to the official Mem integration, so this handle only
/// carries the local memory root used by compatibility constructors.
pub struct MemoryHandle {
    uri: String,
}

impl MemoryHandle {
    pub fn new(uri: &str) -> Self {
        Self {
            uri: uri.to_string(),
        }
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }
}
