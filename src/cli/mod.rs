//! The command-line interface.

mod args;
mod commands;
mod output;
mod prompt;
mod service;
mod text;

use crate::clock::unix_time;
use crate::{
    core::{Cancellation, Core, CoreError, RefreshResult, StatusSnapshot},
    integrations::backend::LiveBackend,
    killmail::displayed_killmails,
};
use args::{Cli, Command, Generate, Service};
use clap::{CommandFactory, Parser};
use commands::{characters, config, listing_store, post, protect};
use output::{emit, emit_error, killmail_details, killmail_table, mail_output, to_json, Output};
use serde_json::json;
use std::{fmt::Write, path::Path, sync::Arc};
use text::{duration, fields, minutes, timestamp, yes_no};

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
    if let Command::Generate(command) = &cli.command {
        return match generate(command) {
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
        Ok(output) => {
            emit(cli.json, &output);
            output.code
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

/// Writes shell completions or man pages generated from the command definition.
fn generate(command: &Generate) -> Result<(), String> {
    match command {
        Generate::Completions { shell } => {
            clap_complete::generate(*shell, &mut Cli::command(), "ekmp", &mut std::io::stdout());
            Ok(())
        }
        Generate::Man { dir } => clap_mangen::generate_to(Cli::command(), dir)
            .map_err(|error| format!("could not write man pages to {}: {error}", dir.display())),
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

fn execute(cli: &Cli, core: &Core, cancel: &Cancellation) -> Result<Output, CoreError> {
    let output = match &cli.command {
        Command::Gui | Command::Generate(_) => {
            unreachable!("handled before a core is created")
        }
        Command::Characters(command) => characters(command, core, cancel)?,
        Command::Refresh => {
            let result = core.refresh(cancel)?;
            let code = u8::from(result.has_failures);
            let text = refresh_text(&result);
            Output::new(to_json(result)?, text).with_code(code)
        }
        Command::List => {
            let (store, now) = (listing_store(cli, core)?, unix_time());
            let mails = displayed_killmails(&store, &store.cached_killmails, now);
            let json = mails
                .iter()
                .map(|mail| mail_output(&store, mail, now, false))
                .collect::<Vec<_>>();
            let mut text = if mails.is_empty() {
                "No unreported killmails to review.\n".to_owned()
            } else {
                killmail_table(&store, &mails, now)
            };
            if !store.show_protected_killmails {
                let mut all = store.clone();
                all.show_protected_killmails = true;
                let hidden =
                    displayed_killmails(&all, &all.cached_killmails, now).len() - mails.len();
                if hidden > 0 {
                    let _ = writeln!(
                        text,
                        "{hidden} protected killmails are hidden; use --show-protected to include them."
                    );
                }
            }
            Output::new(json!(json), text)
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
            Output::new(
                mail_output(&store, mail, now, true),
                killmail_details(&store, mail, now),
            )
        }
        Command::Post(args) => post(args, core, cancel)?,
        Command::Protect(command) => protect(command, core)?,
        Command::Config(command) => config(command, core)?,
        Command::Status => {
            let status = core.snapshot()?.status;
            let text = status_text(&status);
            Output::new(to_json(status)?, text)
        }
        // A stopped service is a clean exit, not a cancelled command.
        Command::Service(Service::Run { interval }) => {
            return service::run(core, *interval, cancel, cli.json);
        }
    };
    let code = if cancel.is_cancelled() {
        130
    } else {
        output.code
    };
    Ok(output.with_code(code))
}

fn refresh_text(result: &RefreshResult) -> String {
    if result.idle {
        return "No characters are authenticated. Add one with `ekmp characters add`.".into();
    }
    if let Some(until) = result.deferred_until {
        let wait = until.saturating_sub(unix_time());
        return format!("Refresh is not due yet; try again in {}.", minutes(wait));
    }
    let mut text: String = result
        .messages
        .iter()
        .map(|message| format!("{message}\n"))
        .collect();
    let verb = if result.reported_found == 1 {
        "was"
    } else {
        "were"
    };
    let _ = write!(
        text,
        "Fetched {} killmails; {} {verb} already on zKillboard.",
        result.fetched_killmails, result.reported_found
    );
    if result.status_checks_incomplete > 0 {
        let _ = write!(
            text,
            " {} status checks are incomplete and will be retried.",
            result.status_checks_incomplete
        );
    }
    text.push('\n');
    text
}

fn status_text(status: &StatusSnapshot) -> String {
    let next = status
        .next_eligible_refresh_at
        .map(|at| {
            let wait = at.saturating_sub(unix_time());
            if wait == 0 {
                "due now".to_owned()
            } else {
                format!("{} (in {})", timestamp(Some(at)), minutes(wait))
            }
        })
        .unwrap_or_else(|| "due now".into());
    let mut pairs = vec![
        ("Characters", status.authenticated_characters.to_string()),
        ("Unreported killmails", status.unreported.to_string()),
        (
            "Awaiting zKillboard status",
            status.awaiting_status.to_string(),
        ),
        (
            "Last successful refresh",
            timestamp(status.last_refresh_success_at),
        ),
        ("Next refresh", next),
        ("Refresh interval", duration(status.refresh_interval_secs)),
        ("Service running", yes_no(status.service_running)),
    ];
    if let Some(error) = &status.last_error {
        pairs.push(("Last error", error.clone()));
    }
    for cooldown in &status.api_cooldowns {
        let scope = cooldown
            .scope
            .as_deref()
            .map(|scope| format!(" ({scope})"))
            .unwrap_or_default();
        pairs.push((
            "API cooldown",
            format!(
                "{}{scope} until {}",
                cooldown.source,
                timestamp(Some(cooldown.until))
            ),
        ));
    }
    fields(&pairs)
}
