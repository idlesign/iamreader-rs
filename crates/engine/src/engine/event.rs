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

/// Wake-up signals carry no state: a slow observer always reads the latest snapshot.
#[derive(Clone, Default)]
pub(super) struct UpdateNotifier {
    subscribers: std::sync::Arc<std::sync::Mutex<Vec<crossbeam_channel::Sender<()>>>>,
}

impl UpdateNotifier {
    pub fn subscribe(&self) -> anyhow::Result<crossbeam_channel::Receiver<()>> {
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let mut subscribers = self.subscribers.lock()
            .map_err(|_| anyhow::anyhow!("Engine update subscription lock poisoned"))?;
        sender.try_send(()).map_err(|error| anyhow::anyhow!("Cannot initialize engine subscription: {error}"))?;
        subscribers.push(sender);

        Ok(receiver)
    }

    pub fn notify(&self) {
        match self.subscribers.lock() {
            Ok(mut subscribers) => subscribers.retain(|sender| {
                !matches!(sender.try_send(()), Err(crossbeam_channel::TrySendError::Disconnected(())))
            }),
            Err(error) => log::error!("Cannot notify engine observers: {error}"),
        }
    }
}
