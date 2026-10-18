use iamreader_engine::project::export_status::{ExportObserver, ExportUpdate};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub(crate) struct BarrierObserver {
    at_publication: bool,
    entered: crossbeam_channel::Sender<()>,
    release: crossbeam_channel::Receiver<()>,
    used: AtomicBool,
}

impl ExportObserver for BarrierObserver {
    fn update(&self, update: ExportUpdate) -> anyhow::Result<()> {
        let at_barrier = match update {
            ExportUpdate::Stage { .. } => !self.at_publication,
            ExportUpdate::Publishing => self.at_publication,
            _ => false,
        };
        if at_barrier && !self.used.swap(true, Ordering::SeqCst) {
            self.entered.send(())?;
            self.release.recv()?;
        }

        Ok(())
    }

    fn is_cancellation_requested(&self) -> anyhow::Result<bool> {
        Ok(false)
    }
}

pub(crate) fn create_barrier(at_publication: bool) -> (Arc<BarrierObserver>, crossbeam_channel::Receiver<()>, crossbeam_channel::Sender<()>) {
    let (entered, received) = crossbeam_channel::bounded(1);
    let (release, released) = crossbeam_channel::bounded(1);

    (Arc::new(BarrierObserver { at_publication, entered, release: released, used: AtomicBool::new(false) }), received, release)
}
