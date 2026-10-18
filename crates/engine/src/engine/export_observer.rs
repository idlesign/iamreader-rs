use crate::project::export_status::{ExportObserver, ExportStatus, ExportUpdate};
use anyhow::Result;
use std::sync::{Arc, Mutex};

pub(super) struct ObservedExport {
    pub status: Arc<Mutex<ExportStatus>>,
    pub listener: Arc<dyn ExportObserver>,
}

impl ExportObserver for ObservedExport {
    fn update(&self, update: ExportUpdate) -> Result<()> {
        self.status.update(update.clone())?;
        self.listener.update(update)
    }

    fn is_cancellation_requested(&self) -> Result<bool> {
        Ok(self.status.is_cancellation_requested()? || self.listener.is_cancellation_requested()?)
    }
}
