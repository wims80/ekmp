//! Interactive input that can be interrupted.

use crate::core::{Cancellation, CoreError};
use std::{
    io::{self, IsTerminal, Write},
    time::Duration,
};

pub(super) fn confirm(yes: bool, prompt: &str, cancel: &Cancellation) -> Result<(), CoreError> {
    if cancel.is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(CoreError::Operational(
            "confirmation requires a terminal; pass --yes to confirm explicitly".into(),
        ));
    }
    eprint!("{prompt} [y/N] ");
    let answer = read_line(cancel)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(CoreError::Cancelled)
    }
}

/// Reads one line from stdin after a prompt on stderr, unless cancelled first.
pub(super) fn read_line(cancel: &Cancellation) -> Result<String, CoreError> {
    io::stderr()
        .flush()
        .map_err(|_| CoreError::Operational("could not display prompt".into()))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut answer = String::new();
        let result = io::stdin().read_line(&mut answer).map(|_| answer);
        let _ = tx.send(result);
    });
    loop {
        if cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(line)) => return Ok(line),
            Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CoreError::Operational("could not read input".into()))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}
