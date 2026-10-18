use crate::project::export_status::{ExportObserver, ExportStatus, ExportUpdate};
use anyhow::Result;
use std::sync::{Arc, Mutex};

pub(super) struct ObservedExport {
    pub status: Arc<Mutex<ExportStatus>>,
    pub listener: Option<Arc<dyn ExportObserver>>,
    pub updates: super::event::UpdateNotifier,
}

impl ExportObserver for ObservedExport {
    fn update(&self, update: ExportUpdate) -> Result<()> {
        self.status.update(update.clone())?;
        self.updates.notify();
        if let Some(listener) = &self.listener { listener.update(update)?; }

        Ok(())
    }

    fn is_cancellation_requested(&self) -> Result<bool> {
        Ok(self.status.is_cancellation_requested()?
            || self.listener.as_ref().map(|listener| listener.is_cancellation_requested()).transpose()?.unwrap_or(false))
    }
}
