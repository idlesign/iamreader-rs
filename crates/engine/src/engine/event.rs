//! Reliable command acknowledgements are separate from coalesced telemetry.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandOutcome {
    Completed,
    Saved { warning: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandFailure {
    pub message: String,
    pub field: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineEvent {
    CommandCompleted {
        request_id: u64,
        result: Result<CommandOutcome, CommandFailure>,
    },
}
