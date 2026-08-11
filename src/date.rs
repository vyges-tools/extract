//! The SPEF `*DATE` stamp: what to write, and where the value comes from.
//!
//! **The standard does not specify a format.** OpenSTA's grammar is `date: DATE QSTRING
//! { stringDelete($2); }` — the token is required in the header sequence, so omitting it is a
//! syntax error, but the string itself is parsed and immediately freed. No consumer reads it.
//! IEEE 1481 treats `*DATE` as provenance, not data.
//!
//! So the only real constraint is convention, and the convention is whatever OpenROAD emits,
//! because that is what every SPEF a user has ever opened looks like:
//!
//! ```text
//! *DATE "17:45:25 Wednesday July 15, 2026"
//! ```
//!
//! That is C `asctime`'s shape — `%H:%M:%S %A %B %d, %Y`. We match it exactly. Deliberately NOT
//! ISO 8601: `tests/spef.rs` pins that, because an ISO stamp in this field looks like a tool that
//! did not read a real SPEF before writing one.
//!
//! **Where the value comes from is the interesting part.** A wall-clock stamp gives provenance
//! but destroys byte-reproducibility; a fixed stamp gives reproducibility but says nothing. The
//! resolution is the one the Reproducible Builds project settled on for exactly this trade-off:
//!
//! 1. an explicit `--date` wins — a caller reproducing a specific run pins it;
//! 2. else `SOURCE_DATE_EPOCH`, the cross-ecosystem env var meaning "pretend it is this instant";
//! 3. else the real current time, in UTC.
//!
//! Provenance by default, determinism on demand — rather than trading one away permanently.
//!
//! This resolution happens in the BINARY, never in [`crate::spef`]. The renderer stays a pure
//! function of its arguments: given `date: None` it emits a fixed stamp and two runs are
//! byte-identical, which is what makes it testable offline with no clock and no environment. A
//! library that reads the wall clock is a library you cannot write a golden test against.

use std::time::{SystemTime, UNIX_EPOCH};

const WEEKDAYS: [&str; 7] = [
    // 1970-01-01 (day 0) was a Thursday, so the table starts there and no offset is needed.
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
];

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Format a Unix timestamp as SPEF's `*DATE` string, in UTC.
///
/// Civil-from-days is Howard Hinnant's algorithm (the one in `<chrono>`'s reference
/// implementation): era-based, exact for the whole range, and no lookup tables or leap-year
/// special cases beyond the era arithmetic. Pure integer maths — this crate carries no date
/// dependency and should not gain one for a header line.
pub fn format_spef_date(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let secs = epoch_secs.rem_euclid(86_400);
    let (h, mi, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);

    // shift the epoch to 0000-03-01 so leap days land at the END of the year
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March-based
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };

    let wd = WEEKDAYS[days.rem_euclid(7) as usize];
    let mo = MONTHS[(m - 1) as usize];
    format!("{h:02}:{mi:02}:{s:02} {wd} {mo} {d:02}, {y}")
}

/// The stamp to write, resolving the precedence documented at the module level.
///
/// `explicit` is the `--date` flag. Returns `None` only when there is no clock to read and no
/// override, which leaves the renderer on its own fixed default.
pub fn resolve(explicit: Option<&str>) -> Option<String> {
    if let Some(d) = explicit {
        return Some(d.to_string());
    }
    // SOURCE_DATE_EPOCH: the reproducible-builds convention. A value we cannot parse is ignored
    // rather than fatal — a broken env var should not fail an extraction, and the fallback is a
    // real timestamp, which is the honest answer.
    if let Ok(v) = std::env::var("SOURCE_DATE_EPOCH") {
        if let Ok(secs) = v.trim().parse::<i64>() {
            return Some(format_spef_date(secs));
        }
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| format_spef_date(d.as_secs() as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The epoch itself, which is also the renderer's fixed default — so the two agree on format
    /// and a reproducible run is indistinguishable from a pinned one.
    #[test]
    fn the_epoch_matches_the_fixed_default() {
        assert_eq!(
            format_spef_date(0),
            "00:00:00 Thursday January 01, 1970",
            "must equal spef.rs's fallback string exactly"
        );
    }

    /// Cross-checked against `date -u -d @<epoch>`; these are the cases that break a hand-rolled
    /// civil-from-days: a leap day, a century non-leap boundary, the day before/after one, and a
    /// pre-epoch (negative) timestamp.
    #[test]
    fn known_instants_round_trip() {
        for (secs, want) in [
            (0_i64, "00:00:00 Thursday January 01, 1970"),
            (1_784_137_525, "17:45:25 Wednesday July 15, 2026"),
            (951_782_400, "00:00:00 Tuesday February 29, 2000"), // leap year (÷400)
            (1_709_164_800, "00:00:00 Thursday February 29, 2024"), // leap year (÷4)
            (4_107_456_000, "00:00:00 Sunday February 28, 2100"), // 2100 is NOT a leap year:
            (4_107_542_400, "00:00:00 Monday March 01, 2100"),   // Feb 28 -> Mar 1, no Feb 29
            (-86_400, "00:00:00 Wednesday December 31, 1969"),   // before the epoch
            (1_735_689_599, "23:59:59 Tuesday December 31, 2024"), // last second of a year
        ] {
            assert_eq!(format_spef_date(secs), want, "epoch {secs}");
        }
    }

    /// An explicit stamp wins over everything, including a set `SOURCE_DATE_EPOCH`.
    #[test]
    fn explicit_wins() {
        assert_eq!(
            resolve(Some("whatever the caller says")).as_deref(),
            Some("whatever the caller says")
        );
    }

    /// The shape a reader expects, whatever the instant: `HH:MM:SS Weekday Month DD, YYYY`.
    /// Asserted structurally rather than against a literal, since this one reads the clock.
    #[test]
    fn a_resolved_stamp_has_the_shape_the_grammar_expects() {
        let s = resolve(None).expect("a clock");
        let (time, rest) = s.split_once(' ').expect("time then date");
        assert_eq!(time.len(), 8, "HH:MM:SS — got {time:?}");
        assert!(
            time.chars().enumerate().all(|(i, c)| if i == 2 || i == 5 {
                c == ':'
            } else {
                c.is_ascii_digit()
            }),
            "HH:MM:SS — got {time:?}"
        );
        let mut parts = rest.split(' ');
        let wd = parts.next().unwrap_or("");
        let mo = parts.next().unwrap_or("");
        assert!(WEEKDAYS.contains(&wd), "weekday — got {wd:?}");
        assert!(MONTHS.contains(&mo), "month — got {mo:?}");
        // The ISO-8601 marker is a `T` BETWEEN DIGITS (2026-08-11T06:07:33), not any `T`.
        // `!s.contains('T')` also matched the weekday, so this failed every Tuesday and
        // Thursday — green on the other five days, which is how it survived.
        assert!(
            !s.as_bytes()
                .windows(3)
                .any(|w| w[0].is_ascii_digit() && w[1] == b'T' && w[2].is_ascii_digit()),
            "must not be ISO 8601 (tests/spef.rs pins this) — got {s:?}"
        );
    }
}
