use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Disjoint cache categories within the inclusive input token count.
/// `None` is unknown; adapters must use `Some(0)` for verified absent categories.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceTokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub cache_write_5m_tokens: Option<u64>,
    pub cache_write_1h_tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferencePurpose {
    Main,
    Summary,
    Classifier,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceAttemptReport {
    pub id: u64,
    pub call_id: u64,
    pub provider: String,
    pub model: String,
    pub duration_ms: u64,
    pub status: InferenceStatus,
    pub usage: Option<InferenceTokenUsage>,
    /// A terminal provider usage receipt, rather than an intermediate stream count.
    pub usage_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceCallReport {
    pub id: u64,
    pub agent_id: u64,
    pub parent_agent_id: Option<u64>,
    pub purpose: InferencePurpose,
    pub status: InferenceStatus,
    pub duration_ms: u64,
    /// False means the backend did not expose its physical attempts.
    pub attempts_observed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceSnapshot {
    pub calls: Vec<InferenceCallReport>,
    pub attempts: Vec<InferenceAttemptReport>,
    /// Work queued or running outside an active model call, including children.
    pub active_agents: u64,
    pub poisoned: bool,
    pub tool_requests: u64,
    pub rejected_tool_requests: u64,
}

impl InferenceSnapshot {
    pub fn is_terminal(&self) -> bool {
        self.active_agents == 0
            && self
                .calls
                .iter()
                .all(|call| call.status != InferenceStatus::Running)
            && self
                .attempts
                .iter()
                .all(|attempt| attempt.status != InferenceStatus::Running)
    }
}

#[derive(Debug, Default)]
struct Ledger {
    snapshot: InferenceSnapshot,
    next_agent_id: u64,
}

/// A live, task-owned ledger; cloning keeps observing the same task.
/// No process-global registry is used, and snapshots contain no request text.
#[derive(Clone, Debug, Default)]
pub struct InferenceTask {
    ledger: Arc<Mutex<Ledger>>,
}

impl InferenceTask {
    fn lock(&self) -> MutexGuard<'_, Ledger> {
        match self.ledger.lock() {
            Ok(ledger) => ledger,
            Err(poisoned) => {
                let mut ledger = poisoned.into_inner();
                ledger.snapshot.poisoned = true;
                ledger
            }
        }
    }

    pub fn snapshot(&self) -> InferenceSnapshot {
        self.lock().snapshot.clone()
    }

    /// Hold this lease from queue admission through the last local/remote action.
    pub fn start_agent(&self, parent_agent_id: Option<u64>) -> InferenceAgent {
        let mut ledger = self.lock();
        let id = ledger.next_agent_id;
        ledger.next_agent_id += 1;
        ledger.snapshot.active_agents += 1;
        InferenceAgent {
            task: self.clone(),
            id,
            parent_agent_id,
        }
    }
}

/// Keeps task completeness false while an agent is queued or running.
#[derive(Debug)]
pub struct InferenceAgent {
    task: InferenceTask,
    id: u64,
    parent_agent_id: Option<u64>,
}

impl InferenceAgent {
    pub fn context(&self) -> InferenceAgentContext {
        InferenceAgentContext {
            task: self.task.clone(),
            id: self.id,
            parent_agent_id: self.parent_agent_id,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn task(&self) -> InferenceTask {
        self.task.clone()
    }

    pub fn start_call(&self, purpose: InferencePurpose) -> InferenceCall {
        self.context().start_call(purpose)
    }
}

/// Explicit context propagated into tools and provider calls; it owns no work lease.
#[derive(Clone, Debug)]
pub struct InferenceAgentContext {
    task: InferenceTask,
    id: u64,
    parent_agent_id: Option<u64>,
}

impl InferenceAgentContext {
    pub fn record_tool_request(&self) {
        self.task.lock().snapshot.tool_requests += 1;
    }

    pub fn record_tool_rejection(&self) {
        self.task.lock().snapshot.rejected_tool_requests += 1;
    }
    pub fn start_child(&self) -> InferenceAgent {
        self.task.start_agent(Some(self.id))
    }

    pub fn start_call(&self, purpose: InferencePurpose) -> InferenceCall {
        let mut ledger = self.task.lock();
        let id = ledger.snapshot.calls.len() as u64;
        ledger.snapshot.calls.push(InferenceCallReport {
            id,
            agent_id: self.id,
            parent_agent_id: self.parent_agent_id,
            purpose,
            status: InferenceStatus::Running,
            duration_ms: 0,
            attempts_observed: false,
        });
        InferenceCall {
            context: InferenceCallContext {
                task: self.task.clone(),
                id,
            },
            started: Instant::now(),
        }
    }
}

impl Drop for InferenceAgent {
    fn drop(&mut self) {
        self.task.lock().snapshot.active_agents -= 1;
    }
}

/// One logical operation; HTTP retries remain children of this call.
#[derive(Debug)]
pub struct InferenceCall {
    context: InferenceCallContext,
    started: Instant,
}

impl InferenceCall {
    pub fn context(&self) -> InferenceCallContext {
        self.context.clone()
    }

    pub fn finish<T, E>(self, result: &Result<T, E>) {
        let mut ledger = self.context.task.lock();
        let call = &mut ledger.snapshot.calls[self.context.id as usize];
        call.duration_ms = elapsed_ms(self.started);
        call.status = if result.is_ok() {
            InferenceStatus::Succeeded
        } else {
            InferenceStatus::Failed
        };
    }
}

impl Drop for InferenceCall {
    fn drop(&mut self) {
        let mut ledger = self.context.task.lock();
        let call = &mut ledger.snapshot.calls[self.context.id as usize];
        if call.status == InferenceStatus::Running {
            call.duration_ms = elapsed_ms(self.started);
            call.status = InferenceStatus::Cancelled;
        }
    }
}

#[derive(Clone, Debug)]
pub struct InferenceCallContext {
    task: InferenceTask,
    id: u64,
}

impl InferenceCallContext {
    /// Begin immediately before a physical request is sent, on every retry.
    pub fn start_attempt(&self, provider: &str, model: &str) -> InferenceAttempt {
        let mut ledger = self.task.lock();
        ledger.snapshot.calls[self.id as usize].attempts_observed = true;
        let id = ledger.snapshot.attempts.len();
        ledger.snapshot.attempts.push(InferenceAttemptReport {
            id: id as u64,
            call_id: self.id,
            provider: provider.to_string(),
            model: model.to_string(),
            duration_ms: 0,
            status: InferenceStatus::Running,
            usage: None,
            usage_complete: false,
        });
        InferenceAttempt {
            task: self.task.clone(),
            id,
            started: Instant::now(),
        }
    }
}

#[derive(Debug)]
pub struct InferenceAttempt {
    task: InferenceTask,
    id: usize,
    started: Instant,
}

impl InferenceAttempt {
    /// Store cumulative provider usage as soon as it arrives, even before EOF.
    pub fn record_usage(&self, usage: InferenceTokenUsage) {
        let mut ledger = self.task.lock();
        let attempt = &mut ledger.snapshot.attempts[self.id];
        if !attempt.usage_complete {
            attempt.usage = Some(usage);
        }
    }

    /// Record only when the protocol identifies the usage as final.
    pub fn record_final_usage(&self, usage: InferenceTokenUsage) {
        let mut ledger = self.task.lock();
        let attempt = &mut ledger.snapshot.attempts[self.id];
        attempt.usage = Some(usage);
        attempt.usage_complete = true;
    }

    pub fn finish<T, E>(self, result: &Result<T, E>) {
        let mut ledger = self.task.lock();
        let attempt = &mut ledger.snapshot.attempts[self.id];
        attempt.duration_ms = elapsed_ms(self.started);
        attempt.status = if result.is_ok() {
            InferenceStatus::Succeeded
        } else {
            InferenceStatus::Failed
        };
    }
}

impl Drop for InferenceAttempt {
    fn drop(&mut self) {
        let mut ledger = self.task.lock();
        let attempt = &mut ledger.snapshot.attempts[self.id];
        if attempt.status == InferenceStatus::Running {
            attempt.duration_ms = elapsed_ms(self.started);
            attempt.status = InferenceStatus::Cancelled;
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
#[path = "inference_tests.rs"]
mod tests;
