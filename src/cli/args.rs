//! Command-line arguments and their value parsers.

use crate::models::MIN_REFRESH_INTERVAL_SECS;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::{path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(
    name = "ekmp",
    version,
    about = "Review and explicitly publish EVE Online killmails",
    arg_required_else_help = true
)]
pub(super) struct Cli {
    #[arg(long, global = true)]
    pub(super) json: bool,
    #[arg(long, global = true, conflicts_with = "hide_protected")]
    pub(super) show_protected: bool,
    #[arg(long, global = true, conflicts_with = "show_protected")]
    pub(super) hide_protected: bool,
    #[arg(long, global = true)]
    pub(super) scenario: Option<String>,
    #[arg(long, global = true, requires = "scenario")]
    pub(super) dev_state: Option<PathBuf>,
    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Subcommand)]
pub(super) enum Command {
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
pub(super) enum Characters {
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
pub(super) struct PostArgs {
    #[arg(required_unless_present = "all", conflicts_with = "all", value_parser = positive_id())]
    pub(super) id: Option<u64>,
    #[arg(long)]
    pub(super) all: bool,
    #[arg(long, requires = "id", conflicts_with = "all")]
    pub(super) post_anyway: bool,
    #[arg(long)]
    pub(super) yes: bool,
}
#[derive(Subcommand)]
pub(super) enum Protect {
    List,
    #[command(subcommand)]
    Add(ProtectAdd),
    #[command(subcommand)]
    Remove(ProtectRemove),
}
#[derive(Subcommand)]
pub(super) enum ProtectAdd {
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
pub(super) enum ProtectRemove {
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
pub(super) enum ConfigKey {
    RefreshInterval,
    ShowProtectedKillmails,
}
#[derive(Subcommand)]
pub(super) enum Config {
    Get {
        key: Option<ConfigKey>,
    },
    #[command(subcommand)]
    Set(Setting),
}
#[derive(Subcommand)]
pub(super) enum Setting {
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
pub(super) enum Service {
    Run {
        #[arg(long, value_parser = parse_interval)]
        interval: Option<Duration>,
    },
}

pub(super) fn positive_id() -> clap::builder::RangedU64ValueParser {
    clap::value_parser!(u64).range(1..)
}

pub(super) fn parse_interval(value: &str) -> Result<Duration, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
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
}
