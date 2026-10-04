use crate::clock::unix_time;
use crate::{
    core::{Cancellation, Core, CoreError, PostSelection},
    integrations::{auth::AuthFlow, backend::LiveBackend},
    killmail::{
        displayed_killmails, is_bulk_candidate, is_eligible_for_bulk_posting, report_state,
        ReportState,
    },
    models::{
        Character, CharacterSource, Killmail, KillmailDetail, ProtectedVictimKind, Store,
        MIN_REFRESH_INTERVAL_SECS,
    },
};
mod service;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
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
    /// Open the desktop interface (requires a build with the gui feature).
    Gui,
    #[command(subcommand)]
    Characters(Characters),
    /// Refresh cached recent killmails and reporting statuses.
    Refresh,
    /// List cached killmails without network requests.
    List,
    /// Show a cached killmail without network requests.
    Show {
        #[arg(value_parser = positive_id())]
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
    /// Authenticate a character with EVE SSO.
    Add {
        /// Print the authorization URL instead of opening a browser.
        #[arg(long)]
        no_browser: bool,
        /// Sign in on any machine, then paste the URL the browser was redirected to.
        #[arg(long, conflicts_with = "no_browser")]
        paste: bool,
    },
    Remove {
        #[arg(value_parser = positive_id())]
        id: u64,
        #[arg(long)]
        yes: bool,
    },
}
#[derive(Args)]
struct PostArgs {
    #[arg(required_unless_present = "all", conflicts_with = "all", value_parser = positive_id())]
    id: Option<u64>,
    #[arg(long)]
    all: bool,
    #[arg(long, requires = "id", conflicts_with = "all")]
    post_anyway: bool,
    #[arg(long)]
    yes: bool,
}
#[derive(Subcommand)]
enum Protect {
    List,
    #[command(subcommand)]
    Add(ProtectAdd),
    #[command(subcommand)]
    Remove(ProtectRemove),
}
#[derive(Subcommand)]
enum ProtectAdd {
    /// Protect a victim character by exact name or EVE ID.
    Character { query: String },
    /// Protect a victim corporation by exact name or EVE ID.
    Corporation { query: String },
    /// Protect one cached killmail.
    Killmail {
        #[arg(value_parser = positive_id())]
        id: u64,
    },
}
#[derive(Subcommand)]
enum ProtectRemove {
    /// Stop protecting a victim character.
    Character {
        #[arg(value_parser = positive_id())]
        id: u64,
    },
    /// Stop protecting a victim corporation.
    Corporation {
        #[arg(value_parser = positive_id())]
        id: u64,
    },
    /// Stop protecting one cached killmail.
    Killmail {
        #[arg(value_parser = positive_id())]
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
    Get {
        key: Option<ConfigKey>,
    },
    #[command(subcommand)]
    Set(Setting),
}
#[derive(Subcommand)]
enum Setting {
    /// Minimum time between refreshes, at least 5m, such as 900s, 15m, or 1h.
    RefreshInterval {
        #[arg(value_parser = parse_interval)]
        value: Duration,
    },
    /// Whether lists include killmails with protected victims.
    ShowProtectedKillmails {
        #[arg(action = clap::ArgAction::Set)]
        value: bool,
    },
}
#[derive(Subcommand)]
enum Service {
    Run {
        #[arg(long, value_parser = parse_interval)]
        interval: Option<Duration>,
    },
}

fn positive_id() -> clap::builder::RangedU64ValueParser {
    clap::value_parser!(u64).range(1..)
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
        .ok_or("interval must be seconds, minutes (15m), or hours (1h)")?;
    if seconds < MIN_REFRESH_INTERVAL_SECS {
        return Err(format!(
            "interval must be at least {}m",
            MIN_REFRESH_INTERVAL_SECS / 60
        ));
    }
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

#[derive(Serialize)]
struct CharacterOutput<'a> {
    id: u64,
    name: &'a str,
    corporation_id: Option<u64>,
    corporation_name: Option<&'a str>,
}

impl<'a> From<&'a Character> for CharacterOutput<'a> {
    fn from(character: &'a Character) -> Self {
        Self {
            id: character.id,
            name: &character.name,
            corporation_id: character.corporation_id,
            corporation_name: character.corporation_name.as_deref(),
        }
    }
}

fn characters(
    command: &Characters,
    core: &Core,
    cancel: &Cancellation,
) -> Result<Output, CoreError> {
    match command {
        Characters::List => {
            let store = core.snapshot()?.store;
            let output = store
                .characters
                .iter()
                .map(CharacterOutput::from)
                .collect::<Vec<_>>();
            Ok((json!(output), 0))
        }
        Characters::Add { no_browser, paste } => {
            let read_pasted_url = || {
                eprint!("Paste the full URL from the browser's address bar: ");
                read_line(cancel).ok()
            };
            let (flow, instructions) = if *paste {
                (AuthFlow::Paste(&read_pasted_url), PASTE_INSTRUCTIONS)
            } else {
                let open_browser = !no_browser && has_display();
                (AuthFlow::Loopback { open_browser }, LOOPBACK_INSTRUCTIONS)
            };
            let character = core.authenticate(cancel, flow, &|url| {
                eprintln!("{instructions}\n\n{url}\n");
            })?;
            Ok((json!({"id": character.id, "name": character.name}), 0))
        }
        Characters::Remove { id, yes } => {
            confirm(
                *yes,
                &format!("Remove character {id}, credentials and unshared cached killmails?"),
                cancel,
            )?;
            let result = core.remove_character(*id)?;
            let code = u8::from(result.credential_warning.is_some());
            Ok((to_json(result)?, code))
        }
    }
}

/// The stored state with this invocation's protected-visibility override applied.
fn listing_store(cli: &Cli, core: &Core) -> Result<Store, CoreError> {
    let mut store = core.snapshot()?.store;
    if cli.show_protected {
        store.show_protected_killmails = true;
    }
    if cli.hide_protected {
        store.show_protected_killmails = false;
    }
    Ok(store)
}

fn post(args: &PostArgs, core: &Core, cancel: &Cancellation) -> Result<Output, CoreError> {
    let selection = match args.id {
        Some(id) if !args.all => PostSelection::One {
            id,
            post_anyway: args.post_anyway,
        },
        _ => PostSelection::All,
    };
    let prepared = core.prepare_post(selection, cancel)?;
    if prepared.ids.is_empty() {
        return Err(CoreError::Operational(
            "no selected killmails are eligible for posting".into(),
        ));
    }
    confirm(
        args.yes,
        &format!(
            "Submit these {} killmail IDs to zKillboard: {:?}?",
            prepared.ids.len(),
            prepared.ids
        ),
        cancel,
    )?;
    let result = match core.post(&prepared, cancel) {
        Ok(result) => result,
        Err(error) => {
            let completed = core.session_reports();
            if completed.is_empty() {
                return Err(error);
            }
            eprintln!("ekmp: {error}");
            let code = if cancel.is_cancelled() { 130 } else { 1 };
            let output =
                json!({"completed": completed, "error": error.to_string(), "exit_code": code});
            return Ok((output, code));
        }
    };
    let code = if result.cancelled {
        130
    } else {
        u8::from(result.has_failures())
    };
    Ok((to_json(result)?, code))
}

fn protect(command: &Protect, core: &Core) -> Result<Value, CoreError> {
    use ProtectedVictimKind::{Character, Corporation};
    match command {
        Protect::List => {
            let store = core.snapshot()?.store;
            let automatic = store
                .characters
                .iter()
                .map(|character| {
                    json!({"id": character.id, "name": character.name, "corporation_id": character.corporation_id})
                })
                .collect::<Vec<_>>();
            Ok(json!({
                "characters": store.manually_protected_characters,
                "corporations": store.manually_protected_corporations,
                "killmail_ids": store.manually_protected_killmail_ids,
                "automatic_characters": automatic,
            }))
        }
        Protect::Add(target) => {
            match target {
                ProtectAdd::Character { query } => {
                    core.add_protected_victim(Character, query)?;
                }
                ProtectAdd::Corporation { query } => {
                    core.add_protected_victim(Corporation, query)?;
                }
                ProtectAdd::Killmail { id } => core.set_killmail_protection(*id, true)?,
            }
            Ok(json!({"protected": true}))
        }
        Protect::Remove(target) => {
            match target {
                ProtectRemove::Character { id } => {
                    core.remove_protected_victim(Character, *id)?;
                }
                ProtectRemove::Corporation { id } => {
                    core.remove_protected_victim(Corporation, *id)?;
                }
                ProtectRemove::Killmail { id } => core.set_killmail_protection(*id, false)?,
            }
            Ok(json!({"removed": true}))
        }
    }
}

fn config(command: &Config, core: &Core) -> Result<Value, CoreError> {
    match command {
        Config::Get { key } => {
            let store = core.snapshot()?.store;
            Ok(match key {
                Some(ConfigKey::RefreshInterval) => {
                    json!({"refresh_interval_secs": store.refresh_interval_secs})
                }
                Some(ConfigKey::ShowProtectedKillmails) => {
                    json!({"show_protected_killmails": store.show_protected_killmails})
                }
                None => json!({
                    "refresh_interval_secs": store.refresh_interval_secs,
                    "show_protected_killmails": store.show_protected_killmails,
                }),
            })
        }
        Config::Set(setting) => {
            match setting {
                Setting::RefreshInterval { value } => core.set_refresh_interval(*value)?,
                Setting::ShowProtectedKillmails { value } => core.set_show_protected(*value)?,
            }
            Ok(json!({"saved": true}))
        }
    }
}

fn to_json(value: impl Serialize) -> Result<Value, CoreError> {
    serde_json::to_value(value)
        .map_err(|_| CoreError::Operational("could not encode output".into()))
}

const LOOPBACK_INSTRUCTIONS: &str = "\
Sign in with this URL in a browser on this machine. From another machine, forward the
callback first (ssh -L 17842:127.0.0.1:17842 HOST) or use `ekmp characters add --paste`.";

const PASTE_INSTRUCTIONS: &str = "\
Open this URL in a browser on any machine and sign in. The browser is then sent to a
127.0.0.1 address that fails to load; that is expected. Copy that address and paste it here.";

/// Whether a graphical session is available to open a browser in.
fn has_display() -> bool {
    ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
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
    let answer = read_line(cancel)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(CoreError::Cancelled)
    }
}

/// Reads one line from stdin after a prompt on stderr, unless cancelled first.
fn read_line(cancel: &Cancellation) -> Result<String, CoreError> {
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
#[derive(Serialize)]
struct KillmailOutput<'a> {
    id: u64,
    sources: &'a [CharacterSource],
    victim_id: Option<u64>,
    victim_corporation_id: Option<u64>,
    victim: &'a str,
    ship: &'a str,
    time: &'a str,
    estimated_value_isk: Option<f64>,
    status: ReportState,
    protected: bool,
    eligible_for_bulk_posting: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a Option<KillmailDetail>>,
}
fn mail_output(store: &Store, mail: &Killmail, now: u64, details: bool) -> Value {
    json!(KillmailOutput {
        id: mail.id,
        sources: &mail.sources,
        victim_id: mail.victim_id,
        victim_corporation_id: mail.victim_corporation_id,
        victim: &mail.victim,
        ship: &mail.ship,
        time: &mail.time,
        estimated_value_isk: mail.estimated_value_isk,
        status: report_state(store, mail.id, now),
        protected: !is_eligible_for_bulk_posting(store, mail),
        eligible_for_bulk_posting: is_bulk_candidate(store, mail, now),
        detail: details.then_some(&mail.detail),
    })
}
fn emit(json_output: bool, value: &Value) {
    if value.is_null() {
        return;
    }
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
    fn intervals_shorter_than_five_minutes_are_rejected() {
        assert!(parse_interval("299").is_err());
        assert!(parse_interval("4m").is_err());
        assert_eq!(parse_interval("300").unwrap().as_secs(), 300);
        assert_eq!(parse_interval("5m").unwrap().as_secs(), 300);
    }
    #[test]
    fn paste_and_no_browser_are_exclusive() {
        assert!(Cli::try_parse_from(["ekmp", "characters", "add", "--paste"]).is_ok());
        assert!(
            Cli::try_parse_from(["ekmp", "characters", "add", "--paste", "--no-browser"]).is_err()
        );
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
        let output = mail_output(&store, &mail, unix_time(), true).to_string();
        assert!(!output.contains("sentinel"));
        assert!(!output.contains("hash"));
        assert!(!output.contains("refresh_token"));
    }
}
