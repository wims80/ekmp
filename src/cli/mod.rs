//! The command-line interface.

mod args;
mod commands;
mod output;
mod prompt;
mod service;

use crate::clock::unix_time;
use crate::{
    core::{Cancellation, Core, CoreError},
    integrations::backend::LiveBackend,
    killmail::displayed_killmails,
};
use args::{Cli, Command, Service};
use clap::{CommandFactory, Parser};
use commands::{characters, config, listing_store, post, protect};
use output::{emit, emit_error, mail_output, to_json};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};

pub(crate) fn run() -> u8 {
    if std::env::args_os().len() == 1 {
        let _ = Cli::command().print_help();
        println!();
        return 0;
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return code;
        }
    };
    if matches!(cli.command, Command::Gui) {
        #[cfg(feature = "gui")]
        let result = crate::app::run(cli.scenario.as_deref(), cli.dev_state.as_deref());
        #[cfg(not(feature = "gui"))]
        let result: Result<(), String> =
            Err("GUI support is unavailable; rebuild with --features gui".into());
        return match result {
            Ok(()) => 0,
            Err(error) => {
                emit_error(cli.json, &error, 1);
                1
            }
        };
    }
    let cancel = Cancellation::new();
    let signal_cancel = cancel.clone();
    if ctrlc::set_handler(move || signal_cancel.cancel()).is_err() {
        emit_error(cli.json, "could not install interruption handler", 1);
        return 1;
    }
    let result = create_core(&cli).and_then(|core| {
        let (tx, rx) = std::sync::mpsc::channel();
        let core = core.with_events(tx);
        let progress = std::thread::spawn(move || {
            for event in rx {
                if let crate::core::CoreEvent::Progress(message) = event {
                    eprintln!("{message}");
                }
            }
        });
        let result = execute(&cli, &core, &cancel);
        drop(core);
        let _ = progress.join();
        result
    });
    match result {
        Ok((value, code)) => {
            emit(cli.json, &value);
            code
        }
        Err(error) => {
            let code = match error {
                CoreError::Busy => 3,
                CoreError::Cancelled => 130,
                _ => 1,
            };
            emit_error(cli.json, &error.to_string(), code);
            code
        }
    }
}

fn create_core(cli: &Cli) -> Result<Core, CoreError> {
    match &cli.scenario {
        None => Core::live(Arc::new(LiveBackend::default())),
        Some(name) => {
            let (core, name) = scenario_core(name, cli.dev_state.as_deref())?;
            eprintln!("Offline simulation: {name}");
            Ok(core)
        }
    }
}

/// Builds a core for an offline scenario, optionally persisted at `dev_state`.
///
/// Returns the core and the scenario's display name.
pub(crate) fn scenario_core(
    name: &str,
    dev_state: Option<&Path>,
) -> Result<(Core, String), CoreError> {
    #[cfg(feature = "dev-tools")]
    {
        let loaded = crate::integrations::simulation::load(name).map_err(CoreError::Operational)?;
        let backend = Arc::new(loaded.backend);
        let core = match dev_state {
            Some(path) => {
                let core = Core::at_path(backend, path.to_path_buf());
                core.initialize(loaded.store)?;
                core
            }
            None => Core::in_memory(backend, loaded.store),
        };
        Ok((core, loaded.name))
    }
    #[cfg(not(feature = "dev-tools"))]
    {
        let _ = dev_state;
        Err(CoreError::Operational(format!(
            "scenario {name:?} requires --features dev-tools"
        )))
    }
}

/// The JSON output and exit code of a successful command.
type Output = (Value, u8);

fn execute(cli: &Cli, core: &Core, cancel: &Cancellation) -> Result<Output, CoreError> {
    let (value, code) = match &cli.command {
        Command::Gui => unreachable!("the GUI is launched before a core is created"),
        Command::Characters(command) => characters(command, core, cancel)?,
        Command::Refresh => {
            let result = core.refresh(cancel)?;
            let code = u8::from(result.has_failures);
            (to_json(result)?, code)
        }
        Command::List => {
            let (store, now) = (listing_store(cli, core)?, unix_time());
            let mails = displayed_killmails(&store, &store.cached_killmails, now);
            let output = mails
                .into_iter()
                .map(|mail| mail_output(&store, mail, now, false))
                .collect::<Vec<_>>();
            (json!(output), 0)
        }
        Command::Show { id } => {
            let (store, now) = (listing_store(cli, core)?, unix_time());
            let mail = displayed_killmails(&store, &store.cached_killmails, now)
                .into_iter()
                .find(|mail| mail.id == *id)
                .ok_or_else(|| {
                    CoreError::Operational(
                        "killmail is missing, reported, or hidden by protection settings".into(),
                    )
                })?;
            (mail_output(&store, mail, now, true), 0)
        }
        Command::Post(args) => post(args, core, cancel)?,
        Command::Protect(command) => (protect(command, core)?, 0),
        Command::Config(command) => (config(command, core)?, 0),
        Command::Status => (to_json(core.snapshot()?.status)?, 0),
        // A stopped service is a clean exit, not a cancelled command.
        Command::Service(Service::Run { interval }) => {
            return service::run(core, *interval, cancel, cli.json);
        }
    };
    Ok((value, if cancel.is_cancelled() { 130 } else { code }))
}
