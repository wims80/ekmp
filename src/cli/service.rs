//! The foreground refresh service run by `ekmp service run`, normally under systemd.
//!
//! It only refreshes: it never posts killmails, authenticates, or opens a browser. Log lines
//! go to stderr without timestamps because journald adds them.

use super::{emit, to_json, Output};
use crate::core::{Cancellation, Core, CoreError, RefreshResult};
use serde_json::Value;
use std::time::Duration;

/// Waits at most this long between checks, so new characters and `config set` changes are
/// picked up without a restart.
const MAX_WAIT: Duration = Duration::from_secs(60);
/// Waits at least this long, e.g. while another command holds the store.
const MIN_WAIT: Duration = Duration::from_secs(5);

pub(super) fn run(
    core: &Core,
    interval: Option<Duration>,
    cancel: &Cancellation,
    json_output: bool,
) -> Result<Output, CoreError> {
    let _guard = core.try_service_guard()?;
    eprintln!("Refresh service started. It refreshes data and never posts killmails.");
    let mut reported_idle = false;
    while !cancel.is_cancelled() {
        match core.refresh_due(interval, cancel) {
            Ok(result) if result.idle => {
                if !reported_idle {
                    eprintln!(
                        "No characters are authenticated; waiting for `ekmp characters add`."
                    );
                    reported_idle = true;
                }
            }
            Ok(result) => {
                reported_idle = false;
                if result.deferred_until.is_none() {
                    report(core, interval, &result, json_output);
                }
            }
            Err(CoreError::Busy) => {}
            Err(CoreError::Cancelled) => break,
            Err(error) => eprintln!("Refresh deferred: {error}"),
        }
        cancel.wait(next_wait(core, interval));
    }
    eprintln!("Refresh service stopped.");
    Ok((Value::Null, 0))
}

fn next_wait(core: &Core, interval: Option<Duration>) -> Duration {
    core.next_refresh_delay(interval)
        .ok()
        .flatten()
        .unwrap_or(MAX_WAIT)
        .clamp(MIN_WAIT, MAX_WAIT)
}

/// Logs one refresh cycle. Reporting is best-effort and never stops the service.
fn report(core: &Core, interval: Option<Duration>, result: &RefreshResult, json_output: bool) {
    if json_output {
        if let Ok(value) = to_json(result) {
            emit(true, &value);
        }
        return;
    }
    for message in &result.messages {
        eprintln!("{message}");
    }
    let outcome = if result.has_failures {
        "Refresh finished with errors"
    } else {
        "Refresh complete"
    };
    let counts = core
        .snapshot()
        .map(|snapshot| {
            format!(
                ": {} unreported, {} awaiting zKillboard status",
                snapshot.status.unreported, snapshot.status.awaiting_status
            )
        })
        .unwrap_or_default();
    let next = core
        .next_refresh_delay(interval)
        .ok()
        .flatten()
        .map(|delay| format!("; next in {}", minutes(delay)))
        .unwrap_or_default();
    eprintln!("{outcome}{counts}{next}.");
}

/// Formats `delay` as whole minutes, rounded up so a pending refresh never shows as `0m`.
fn minutes(delay: Duration) -> String {
    format!("{}m", delay.as_secs().div_ceil(60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minutes_round_up() {
        assert_eq!(minutes(Duration::from_secs(1)), "1m");
        assert_eq!(minutes(Duration::from_secs(900)), "15m");
        assert_eq!(minutes(Duration::from_secs(901)), "16m");
    }
}
