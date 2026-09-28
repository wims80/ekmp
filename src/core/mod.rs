mod characters;
mod posting;
mod protection;
mod refresh;
mod status;
mod store;
#[cfg(test)]
mod test_support;
mod timing;
mod types;

pub(crate) use store::ServiceGuard;
pub(crate) use timing::Cancellation;
#[cfg(feature = "gui")]
pub(crate) use types::CredentialMigrationResult;
pub(crate) use types::{
    CoreError, CoreEvent, CoreResult, PostBatchResult, PostMode, PostResult, PostResultStatus,
    PostSelection, PreparedPost, RefreshResult, RemoveCharacterResult, SessionReport, Snapshot,
    StatusSnapshot,
};

use crate::integrations::backend::Backend;
use std::sync::{mpsc::Sender, Arc, Mutex};

use store::Persistence;

#[derive(Clone)]
pub(crate) struct Core {
    backend: Arc<dyn Backend>,
    persistence: Persistence,
    events: Option<Sender<CoreEvent>>,
    session_reports: Arc<Mutex<Vec<SessionReport>>>,
}

impl Core {
    pub(crate) fn with_events(mut self, events: Sender<CoreEvent>) -> Self {
        self.events = Some(events);
        self
    }

    fn progress(&self, message: impl Into<String>) {
        if let Some(events) = &self.events {
            let _ = events.send(CoreEvent::Progress(message.into()));
        }
    }

    fn changed(&self) {
        if let Some(events) = &self.events {
            let _ = events.send(CoreEvent::SnapshotChanged);
        }
    }
}
