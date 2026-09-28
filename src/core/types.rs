use crate::models::{ApiCooldown, Store};
use serde::Serialize;
use std::fmt;

pub(crate) type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoreError {
    Busy,
    Cancelled,
    Persistence(String),
    Operational(String),
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => formatter.write_str("another ekmp operation is already in progress"),
            Self::Cancelled => formatter.write_str("operation cancelled"),
            Self::Persistence(error) | Self::Operational(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for CoreError {}

#[derive(Clone, Debug)]
pub(crate) enum CoreEvent {
    Progress(String),
    SnapshotChanged,
}

#[derive(Clone)]
pub(crate) struct Snapshot {
    pub(crate) store: Store,
    pub(crate) status: StatusSnapshot,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct StatusSnapshot {
    pub(crate) authenticated_characters: usize,
    pub(crate) cached_killmails: usize,
    pub(crate) reported: usize,
    pub(crate) unreported: usize,
    pub(crate) awaiting_status: usize,
    pub(crate) last_refresh_attempt_at: Option<u64>,
    pub(crate) last_refresh_success_at: Option<u64>,
    pub(crate) last_refresh_completed_at: Option<u64>,
    pub(crate) next_eligible_refresh_at: Option<u64>,
    pub(crate) refresh_interval_secs: u64,
    pub(crate) stale: bool,
    pub(crate) last_error: Option<String>,
    pub(crate) service_running: bool,
    pub(crate) api_cooldowns: Vec<ApiCooldown>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RefreshResult {
    pub(crate) fetched_killmails: usize,
    pub(crate) reported_found: usize,
    pub(crate) status_checks_incomplete: usize,
    pub(crate) idle: bool,
    pub(crate) deferred_until: Option<u64>,
    pub(crate) messages: Vec<String>,
    pub(crate) has_failures: bool,
}

impl RefreshResult {
    pub(crate) fn has_failures(&self) -> bool {
        self.has_failures || self.status_checks_incomplete > 0
    }

    pub(super) fn idle() -> Self {
        Self {
            fetched_killmails: 0,
            reported_found: 0,
            status_checks_incomplete: 0,
            idle: true,
            deferred_until: None,
            messages: Vec::new(),
            has_failures: false,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RemoveCharacterResult {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) removed_killmails: usize,
    pub(crate) credential_warning: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg(feature = "gui")]
pub(crate) struct CredentialMigrationResult {
    pub(crate) migrated: usize,
    pub(crate) warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PostSelection {
    One { id: u64, post_anyway: bool },
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum PostMode {
    Individual,
    ProtectedIndividual,
    Bulk,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedPost {
    pub(crate) ids: Vec<u64>,
    pub(crate) mode: PostMode,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PostBatchResult {
    pub(crate) results: Vec<PostResult>,
    pub(crate) cancelled: bool,
}

impl PostBatchResult {
    pub(crate) fn has_failures(&self) -> bool {
        self.results.iter().any(|result| {
            matches!(
                result.status,
                PostResultStatus::Failed | PostResultStatus::Skipped
            )
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PostResult {
    pub(crate) killmail_id: u64,
    pub(crate) status: PostResultStatus,
    pub(crate) url: Option<String>,
    pub(crate) message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PostResultStatus {
    Submitted,
    AlreadyPresent,
    Skipped,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SessionReport {
    pub(crate) killmail_id: u64,
    pub(crate) url: String,
    pub(crate) new: bool,
}
