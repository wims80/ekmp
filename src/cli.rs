use crate::{
    core::{Cancellation, Core, CoreError, PostSelection},
    integrations::backend::LiveBackend,
    killmail::{displayed_killmails, protection_reasons, report_state, ReportState},
    models::{Killmail, ProtectedVictimKind, Store},
};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};
use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

#[derive(Parser)]
#[command(
    name = "ekmp",
    version,
    about = "Review and explicitly publish EVE Online killmails",
    arg_required_else_help = true
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true, conflicts_with = "hide_protected")]
    show_protected: bool,
    #[arg(long, global = true, conflicts_with = "show_protected")]
    hide_protected: bool,
    #[arg(long, global = true)]
    scenario: Option<String>,
    #[arg(long, global = true, requires = "scenario")]
    dev_state: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open the desktop interface (requires the gui build feature).
    Gui,
    #[command(subcommand)]
    Characters(Characters),
    /// Refresh cached recent killmails and reporting statuses.
    Refresh,
    /// List cached killmails without network requests.
    List,
    /// Show a cached killmail without network requests.
    Show {
        #[arg(value_parser = clap::value_parser!(u64).range(1..))]
        id: u64,
    },
    /// Explicitly submit selected confirmed-unreported killmails.
    Post(PostArgs),
    #[command(subcommand)]
    Protect(Protect),
    #[command(subcommand)]
    Config(Config),
    #[command(subcommand)]
    Service(Service),
    /// Show cached counts, refresh schedule and service status.
    Status,
}
#[derive(Subcommand)]
enum Characters {
    List,
    Add {
        #[arg(long)]
        no_browser: bool,
    },
    Remove {
        #[arg(value_parser = clap::value_parser!(u64).range(1..))]
        id: u64,
        #[arg(long)]
        yes: bool,
    },
}
#[derive(Args)]
struct PostArgs {
    #[arg(required_unless_present = "all", conflicts_with = "all", value_parser = clap::value_parser!(u64).range(1..))]
    id: Option<u64>,
    #[arg(long)]
    all: bool,
    #[arg(long, requires = "id", conflicts_with = "all")]
    post_anyway: bool,
    #[arg(long)]
    yes: bool,
}
#[derive(Clone, Copy, ValueEnum)]
enum ProtectionKind {
    Character,
    Corporation,
    Killmail,
}
#[derive(Subcommand)]
enum Protect {
    List,
    Add {
        kind: ProtectionKind,
        query: String,
    },
    Remove {
        kind: ProtectionKind,
        #[arg(value_parser = clap::value_parser!(u64).range(1..))]
        id: u64,
    },
}
#[derive(Clone, Copy, ValueEnum)]
enum ConfigKey {
    RefreshInterval,
    ShowProtectedKillmails,
}
#[derive(Subcommand)]
enum Config {
    Get { key: Option<ConfigKey> },
    Set { key: ConfigKey, value: String },
}
#[derive(Subcommand)]
enum Service {
    Run {
        #[arg(long, value_parser = parse_interval)]
        interval: Option<Duration>,
    },
}

fn parse_interval(value: &str) -> Result<Duration, String> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix('s') {
        (number, 1)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 3600)
    } else {
        (value, 1)
    };
    let seconds = number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .filter(|n| *n > 0)
        .ok_or("interval must be positive seconds, minutes (15m), or hours (1h)")?;
    Ok(Duration::from_secs(seconds))
}

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
    if let Err(message) = validate_values(&cli.command) {
        emit_error(cli.json, &message, 2);
        return 2;
    }
    if matches!(cli.command, Command::Gui) {
        #[cfg(feature = "gui")]
        let result = crate::launch_gui(cli.scenario.clone(), cli.dev_state.clone());
        #[cfg(not(feature = "gui"))]
        let result: Result<(), String> =
            Err("GUI support is unavailable; install a default build".into());
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

fn validate_values(command: &Command) -> Result<(), String> {
    match command {
        Command::Config(Config::Set {
            key: ConfigKey::RefreshInterval,
            value,
        }) => {
            parse_interval(value)?;
        }
        Command::Config(Config::Set {
            key: ConfigKey::ShowProtectedKillmails,
            value,
        }) => {
            value
                .parse::<bool>()
                .map_err(|_| "value must be true or false")?;
        }
        Command::Protect(Protect::Add {
            kind: ProtectionKind::Killmail,
            query,
        }) => {
            query
                .parse::<u64>()
                .ok()
                .filter(|id| *id > 0)
                .ok_or("killmail selection requires a positive numeric ID")?;
        }
        _ => {}
    }
    Ok(())
}

fn create_core(cli: &Cli) -> Result<Core, CoreError> {
    match &cli.scenario {
        None => Core::live(Arc::new(LiveBackend)),
        Some(name) => {
            #[cfg(feature = "dev-tools")]
            {
                let loaded =
                    crate::integrations::simulation::load(name).map_err(CoreError::Operational)?;
                eprintln!("Offline simulation: {}", loaded.name);
                let backend = Arc::new(loaded.backend);
                if let Some(path) = &cli.dev_state {
                    let core = Core::at_path(backend, path.clone());
                    core.initialize(loaded.store)?;
                    Ok(core)
                } else {
                    Ok(Core::in_memory(backend, loaded.store))
                }
            }
            #[cfg(not(feature = "dev-tools"))]
            {
                Err(CoreError::Operational(format!(
                    "scenario {name:?} requires --features dev-tools"
                )))
            }
        }
    }
}

fn execute(cli: &Cli, core: &Core, cancel: &Cancellation) -> Result<(Value, u8), CoreError> {
    let value = match &cli.command {
        Command::Gui => unreachable!(),
        Command::Characters(command) => match command {
            Characters::List => json!(core.snapshot()?.store.characters.iter().map(|c| json!({"id":c.id,"name":c.name,"corporation_id":c.corporation_id,"corporation_name":c.corporation_name})).collect::<Vec<_>>()),
            Characters::Add { no_browser } => { let c = core.authenticate(cancel, !no_browser, &|url| eprintln!("Authorize on this machine: {url}"))?; json!({"id":c.id,"name":c.name}) },
            Characters::Remove { id, yes } => {
                confirm(*yes, &format!("Remove character {id}, credentials and unshared cached killmails?"), cancel)?;
                let result = core.remove_character(*id)?;
                let code = u8::from(result.credential_warning.is_some());
                return Ok((serde_json::to_value(result).map_err(output_error)?, code));
            }
        },
        Command::Refresh => {
            let result = core.refresh(cancel)?;
            let code = u8::from(result.has_failures());
            return Ok((serde_json::to_value(result).map_err(output_error)?, if cancel.is_cancelled() {130} else {code}));
        },
        Command::List | Command::Show { .. } => {
            let mut store = core.snapshot()?.store;
            if cli.show_protected { store.show_protected_killmails = true; }
            if cli.hide_protected { store.show_protected_killmails = false; }
            let now = now();
            let mails = displayed_killmails(&store, &store.cached_killmails, now);
            match &cli.command {
                Command::Show { id } => { let mail = mails.into_iter().find(|m| m.id == *id).ok_or_else(|| CoreError::Operational("killmail is missing, reported, or hidden by protection settings".into()))?; mail_output(&store, mail, now, true) },
                _ => json!(mails.into_iter().map(|mail| mail_output(&store,mail,now,false)).collect::<Vec<_>>()),
            }
        }
        Command::Post(args) => {
            let selection = if args.all { PostSelection::All } else { PostSelection::One { id: args.id.expect("clap requires ID"), post_anyway: args.post_anyway } };
            let prepared = core.prepare_post(selection, cancel)?;
            if prepared.ids.is_empty() { return Err(CoreError::Operational("no selected killmails are eligible for posting".into())); }
            confirm(args.yes, &format!("Submit these {} killmail IDs to zKillboard: {:?}?", prepared.ids.len(), prepared.ids), cancel)?;
            let result = match core.post(&prepared, cancel) {
                Ok(result) => result,
                Err(error) => {
                    let completed = core.session_reports();
                    if completed.is_empty() { return Err(error); }
                    eprintln!("ekmp: {error}");
                    let code = if cancel.is_cancelled() { 130 } else { 1 };
                    return Ok((json!({"completed":completed,"error":error.to_string(),"exit_code":code}), code));
                }
            };
            let code = if result.cancelled || cancel.is_cancelled() { 130 } else if result.has_failures() { 1 } else { 0 };
            let value = serde_json::to_value(result).map_err(output_error)?;
            return Ok((value, code));
        }
        Command::Protect(command) => match command {
            Protect::List => { let store = core.snapshot()?.store; json!({"characters":store.manually_protected_characters,"corporations":store.manually_protected_corporations,"killmail_ids":store.manually_protected_killmail_ids,"automatic_characters":store.characters.iter().map(|c| json!({"id":c.id,"name":c.name,"corporation_id":c.corporation_id})).collect::<Vec<_>>()}) },
            Protect::Add { kind, query } => {
                match kind {
                    ProtectionKind::Killmail => { let id = query.parse().map_err(|_| CoreError::Operational("killmail selection requires a numeric ID".into()))?; core.set_killmail_protection(id, true)?; },
                    _ => { core.add_protected_victim(victim_kind(*kind), query)?; }
                }; json!({"protected":true})
            },
            Protect::Remove { kind, id } => {
                match kind { ProtectionKind::Killmail => { core.set_killmail_protection(*id,false)?; }, _ => { core.remove_protected_victim(victim_kind(*kind),*id)?; } }; json!({"removed":true})
            }
        },
        Command::Config(command) => match command {
            Config::Get { key } => { let store = core.snapshot()?.store; match key {
                Some(ConfigKey::RefreshInterval) => json!({"refresh_interval_secs":store.refresh_interval_secs}),
                Some(ConfigKey::ShowProtectedKillmails) => json!({"show_protected_killmails":store.show_protected_killmails}),
                None => json!({"refresh_interval_secs":store.refresh_interval_secs,"show_protected_killmails":store.show_protected_killmails})
            } },
            Config::Set { key, value } => { match key {
                ConfigKey::RefreshInterval => core.set_refresh_interval(parse_interval(value).map_err(CoreError::Operational)?)?,
                ConfigKey::ShowProtectedKillmails => core.set_show_protected(value.parse().map_err(|_| CoreError::Operational("value must be true or false".into()))?)?,
            }; json!({"saved":true}) }
        },
        Command::Status => serde_json::to_value(core.snapshot()?.status).map_err(output_error)?,
        Command::Service(Service::Run { interval }) => return service(core, *interval, cancel, cli.json),
    };
    Ok((value, if cancel.is_cancelled() { 130 } else { 0 }))
}

fn service(
    core: &Core,
    interval: Option<Duration>,
    cancel: &Cancellation,
    json_output: bool,
) -> Result<(Value, u8), CoreError> {
    let _guard = core.try_service_guard()?;
    eprintln!("Refresh service running; press Ctrl+C to stop.");
    while !cancel.is_cancelled() {
        match core.refresh_due(interval, cancel) {
            Ok(result) => {
                if json_output && !result.idle && result.deferred_until.is_none() {
                    emit(true, &serde_json::to_value(result).map_err(output_error)?);
                }
            }
            Err(CoreError::Busy) => {}
            Err(CoreError::Cancelled) => break,
            Err(error) => eprintln!("Refresh deferred: {error}"),
        }
        let delay = match core.next_refresh_delay(interval) {
            Ok(Some(delay)) => delay.max(Duration::from_secs(1)),
            _ => Duration::from_secs(2),
        };
        let delay = delay.min(Duration::from_secs(2));
        let start = std::time::Instant::now();
        while !cancel.is_cancelled() && start.elapsed() < delay {
            std::thread::sleep(
                Duration::from_millis(100).min(delay.saturating_sub(start.elapsed())),
            );
        }
    }
    Ok((json!({"service":"stopped"}), 130))
}

fn confirm(yes: bool, prompt: &str, cancel: &Cancellation) -> Result<(), CoreError> {
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
    io::stderr()
        .flush()
        .map_err(|_| CoreError::Operational("could not display confirmation".into()))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut answer = String::new();
        let result = io::stdin().read_line(&mut answer).map(|_| answer);
        let _ = tx.send(result);
    });
    let answer = loop {
        if cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(answer)) => break answer,
            Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CoreError::Operational("could not read confirmation".into()))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    };
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(CoreError::Cancelled)
    }
}
fn victim_kind(kind: ProtectionKind) -> ProtectedVictimKind {
    match kind {
        ProtectionKind::Character => ProtectedVictimKind::Character,
        ProtectionKind::Corporation => ProtectedVictimKind::Corporation,
        ProtectionKind::Killmail => unreachable!(),
    }
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn output_error(_: serde_json::Error) -> CoreError {
    CoreError::Operational("could not encode output".into())
}
fn mail_output(store: &Store, mail: &Killmail, now: u64, details: bool) -> Value {
    let status = match report_state(store, mail.id, now) {
        ReportState::Reported => "reported",
        ReportState::Unreported => "unreported",
        ReportState::Unknown => "unknown",
    };
    let protected = !protection_reasons(store, mail).is_empty();
    let mut value = json!({"id":mail.id,"sources":mail.sources,"victim_id":mail.victim_id,"victim_corporation_id":mail.victim_corporation_id,"victim":mail.victim,"ship":mail.ship,"time":mail.time,"estimated_value_isk":mail.estimated_value_isk,"status":status,"protected":protected,"eligible_for_bulk_posting":!protected && status == "unreported"});
    if details {
        value["detail"] = json!(mail.detail);
    }
    value
}
fn emit(json_output: bool, value: &Value) {
    if json_output {
        println!("{value}");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
    }
}
fn emit_error(json_output: bool, message: &str, code: u8) {
    if json_output {
        println!("{}", json!({"error":message,"exit_code":code}));
    }
    eprintln!("ekmp: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
    #[test]
    fn posting_flags_do_not_allow_bulk_override() {
        assert!(Cli::try_parse_from(["ekmp", "post", "--all", "--post-anyway"]).is_err());
    }
    #[test]
    fn intervals_reject_zero_overflow_and_invalid_units() {
        for input in ["0", "0m", "-1", "1d", "18446744073709551615h"] {
            assert!(parse_interval(input).is_err());
        }
        assert_eq!(parse_interval("15m").unwrap().as_secs(), 900);
    }
    #[test]
    fn dev_state_requires_scenario() {
        assert!(Cli::try_parse_from(["ekmp", "--dev-state", "x", "list"]).is_err());
    }
    #[test]
    fn killmail_output_excludes_hashes_and_credentials() {
        let mail = Killmail {
            id: 1,
            hash: "sentinel-private-hash".into(),
            sources: vec![],
            victim_id: None,
            victim_corporation_id: None,
            victim: "Victim".into(),
            ship: "Ship".into(),
            time: "2026-01-01T00:00:00Z".into(),
            estimated_value_isk: None,
            detail: None,
        };
        let mut store = Store::default();
        store.characters.push(crate::models::Character {
            id: 2,
            name: "Pilot".into(),
            refresh_token: Some("sentinel-private-token".into()),
            corporation_id: None,
            corporation_name: None,
        });
        let output = mail_output(&store, &mail, now(), true).to_string();
        assert!(!output.contains("sentinel"));
        assert!(!output.contains("hash"));
        assert!(!output.contains("refresh_token"));
    }
}
