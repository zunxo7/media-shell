//! Dates and times as the apps write them. The core has no clock or time zone: the app passes the time now and its
//! offset from UTC.

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

/// The local day number (days since 1 January 1970) of `at` seconds since 1970, `offset` seconds east of UTC.
pub fn day(at: f64, offset: i32) -> i64 {
    ((at + f64::from(offset)) / 86_400.).floor() as i64
}

/// (year, month 1–12, day 1–31) of a day number: Howard Hinnant's `civil_from_days`.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The local year of `at` seconds since 1970, `offset` seconds east of UTC.
pub fn year(at: f64, offset: i32) -> i64 {
    civil(day(at, offset)).0
}

/// An episode's air date as the server sends it ("2024-03-07", maybe with a time after) as "7 Mar 2024".
pub fn short_date(value: &str) -> Option<String> {
    let mut parts = value.get(..10)?.split('-');
    let (year, month, n) = (parts.next()?, parts.next()?.parse::<usize>().ok()?, parts.next()?.parse::<u32>().ok()?);
    let month = MONTHS.get(month.checked_sub(1)?)?.get(..3)?;
    (1..=31).contains(&n).then(|| format!("{n} {month} {year}"))
}

/// A day's heading: "Today", "Yesterday", then the date ("3rd November", with the year when it isn't this one).
pub fn day_heading(day: i64, today: i64) -> String {
    if day >= today {
        return "Today".into();
    }
    if day == today - 1 {
        return "Yesterday".into();
    }
    let ((year, month, n), (this_year, ..)) = (civil(day), civil(today));
    let th = match (n % 10, n) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    let month = MONTHS[(month as usize).saturating_sub(1).min(11)];
    if year == this_year {
        format!("{n}{th} {month}")
    } else {
        format!("{n}{th} {month} {year}")
    }
}

/// The time of day: "9:41 pm", "12:05 am".
pub fn time_of_day(at: f64, offset: i32) -> String {
    let seconds = (at + f64::from(offset)).rem_euclid(86_400.) as i64;
    let (h, m) = (seconds / 3600, seconds % 3600 / 60);
    let twelve = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{twelve}:{m:02} {}", if h < 12 { "am" } else { "pm" })
}

/// A title not out yet, on its card: "Coming 15 Mar" (the year too when it isn't this one). None once it's out.
pub fn coming(available_at: Option<i64>, now: f64, offset: i32) -> Option<String> {
    let at = available_at.filter(|&at| at as f64 > now)? as f64;
    let ((year, month, n), (this_year, ..)) = (civil(day(at, offset)), civil(day(now, offset)));
    let month = &MONTHS[(month as usize).saturating_sub(1).min(11)][..3];
    Some(if year == this_year { format!("Coming {n} {month}") } else { format!("Coming {n} {month} {year}") })
}

/// How long until a title is out, on its Play button: "Available in 3d 4h", "Available in 5h 12m",
/// "Available in 40s". None once it's out.
pub fn countdown(available_at: Option<i64>, now: f64) -> Option<String> {
    let left = (available_at? as f64 - now).ceil() as i64;
    if left <= 0 {
        return None;
    }
    let (d, h, m, s) = (left / 86_400, left % 86_400 / 3600, left % 3600 / 60, left % 60);
    let text = match (d, h, m) {
        (0, 0, 0) => format!("{s}s"),
        (0, 0, _) => format!("{m}m {s}s"),
        (0, _, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    };
    Some(format!("Available in {text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coming_and_counting_down() {
        assert_eq!(coming(Some(NOON as i64 + 86_400), NOON, 0).as_deref(), Some("Coming 15 Nov"));
        assert_eq!(coming(Some(NOON as i64 + 60 * 86_400), NOON, 0).as_deref(), Some("Coming 13 Jan 2027"));
        assert_eq!(coming(Some(NOON as i64 - 1), NOON, 0), None);
        assert_eq!(coming(None, NOON, 0), None);
        let at = Some(NOON as i64 + 3 * 86_400 + 4 * 3600 + 5);
        assert_eq!(countdown(at, NOON).as_deref(), Some("Available in 3d 4h"));
        assert_eq!(countdown(Some(NOON as i64 + 5 * 3600 + 12 * 60), NOON).as_deref(), Some("Available in 5h 12m"));
        assert_eq!(countdown(Some(NOON as i64 + 40), NOON).as_deref(), Some("Available in 40s"));
        assert_eq!(countdown(Some(NOON as i64), NOON), None);
    }

    #[test]
    fn years_and_short_dates() {
        assert_eq!(year(1_794_657_600., 0), 2026);
        assert_eq!(year(1_798_761_599., 0), 2026);
        assert_eq!(year(1_798_761_599., 3600), 2027);
        assert_eq!(short_date("2024-03-07T10:00:00Z").as_deref(), Some("7 Mar 2024"));
        assert_eq!(short_date("2024-13-07"), None);
        assert_eq!(short_date("2024-03"), None);
    }

    // 14 Nov 2026 12:00 UTC
    const NOON: f64 = 1_794_657_600.;

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(day(NOON, 0)), (2026, 11, 14));
        assert_eq!(civil(day(951_782_400., 0)), (2000, 2, 29));
    }

    #[test]
    fn headings() {
        let today = day(NOON, 0);
        assert_eq!(day_heading(today, today), "Today");
        assert_eq!(day_heading(today - 1, today), "Yesterday");
        assert_eq!(day_heading(today - 11, today), "3rd November");
        assert_eq!(day_heading(today - 3, today), "11th November");
        assert_eq!(day_heading(today - 13, today), "1st November");
        assert_eq!(day_heading(today - 365, today), "14th November 2025");
    }

    #[test]
    fn local_day_and_time_follow_the_offset() {
        // 23:30 UTC is the next day in Lahore (+5) and the same day in New York (-5)
        let late = NOON + 11.5 * 3600.;
        assert_eq!(day(late, 5 * 3600), day(NOON, 0) + 1);
        assert_eq!(day(late, -5 * 3600), day(NOON, 0));
        assert_eq!(time_of_day(late, 5 * 3600), "4:30 am");
        assert_eq!(time_of_day(NOON, 0), "12:00 pm");
        assert_eq!(time_of_day(NOON - 12. * 3600. + 300., 0), "12:05 am");
        assert_eq!(time_of_day(NOON + 9. * 3600. + 41. * 60., 0), "9:41 pm");
    }
}
