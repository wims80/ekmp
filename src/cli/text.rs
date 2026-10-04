//! Formatting helpers for human-readable output.

use crate::clock::format_utc;
use std::fmt::Write;

/// Left-aligned columns separated by two spaces, with a header row.
pub(super) fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| header.chars().count())
        .collect();
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut output = String::new();
    let header_row: Vec<String> = headers.iter().map(|header| (*header).to_owned()).collect();
    for row in std::iter::once(&header_row).chain(rows) {
        let mut line = String::new();
        for (index, (cell, width)) in row.iter().zip(&widths).enumerate() {
            if index > 0 {
                line.push_str("  ");
            }
            let _ = write!(line, "{cell:width$}");
        }
        output.push_str(line.trim_end());
        output.push('\n');
    }
    output
}

/// `label: value` lines with the values aligned.
pub(super) fn fields(pairs: &[(&str, String)]) -> String {
    let width = pairs
        .iter()
        .map(|(label, _)| label.chars().count() + 1)
        .max()
        .unwrap_or(0);
    pairs
        .iter()
        .map(|(label, value)| format!("{:width$} {value}\n", format!("{label}:")))
        .collect()
}

/// An ISK amount with a k/M/B suffix, or `-` when unknown.
pub(super) fn isk(value: Option<f64>) -> String {
    let Some(value) = value else {
        return "-".into();
    };
    match value.abs() {
        v if v >= 1e9 => format!("{:.2}B", value / 1e9),
        v if v >= 1e6 => format!("{:.2}M", value / 1e6),
        v if v >= 1e3 => format!("{:.1}k", value / 1e3),
        _ => format!("{value:.0}"),
    }
}

/// An ESI killmail time (`2026-08-16T10:00:00Z`) as `2026-08-16 10:00`.
pub(super) fn killmail_time(time: &str) -> String {
    match (time.get(..10), time.get(11..16)) {
        (Some(date), Some(clock)) if time.as_bytes().get(10) == Some(&b'T') => {
            format!("{date} {clock}")
        }
        _ => time.to_owned(),
    }
}

/// Unix seconds in UTC, or `never`.
pub(super) fn timestamp(secs: Option<u64>) -> String {
    secs.map_or_else(|| "never".into(), format_utc)
}

/// Seconds as the largest whole unit: `1h`, `15m`, or `90s`.
pub(super) fn duration(secs: u64) -> String {
    match secs {
        s if s > 0 && s % 3600 == 0 => format!("{}h", s / 3600),
        s if s > 0 && s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// Whole minutes, rounded up so a pending event never shows as `0m`.
pub(super) fn minutes(secs: u64) -> String {
    format!("{}m", secs.div_ceil(60))
}

pub(super) fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_aligns_columns_by_character_count() {
        let output = table(
            &["ID", "NAME"],
            &[
                vec!["1".into(), "Ærø".into()],
                vec!["1234".into(), "Pilot".into()],
            ],
        );

        assert_eq!(output, "ID    NAME\n1     Ærø\n1234  Pilot\n");
    }

    #[test]
    fn fields_align_values() {
        assert_eq!(
            fields(&[("A", "1".into()), ("Longer", "2".into())]),
            "A:      1\nLonger: 2\n"
        );
    }

    #[test]
    fn isk_uses_suffixes() {
        assert_eq!(isk(None), "-");
        assert_eq!(isk(Some(950.0)), "950");
        assert_eq!(isk(Some(12_500.0)), "12.5k");
        assert_eq!(isk(Some(1_250_000.0)), "1.25M");
        assert_eq!(isk(Some(3_400_000_000.0)), "3.40B");
    }

    #[test]
    fn killmail_time_is_shortened_only_when_well_formed() {
        assert_eq!(killmail_time("2026-08-16T10:00:00Z"), "2026-08-16 10:00");
        assert_eq!(killmail_time("Time"), "Time");
    }

    #[test]
    fn durations_use_the_largest_whole_unit() {
        assert_eq!(duration(3600), "1h");
        assert_eq!(duration(900), "15m");
        assert_eq!(duration(90), "90s");
        assert_eq!(minutes(1), "1m");
        assert_eq!(minutes(901), "16m");
    }
}
