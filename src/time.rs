//! Strict date-time parsing shared by Launchpad and CLI inputs.

use jiff::{Timestamp, tz::Offset};

/// Parse a strict RFC 3339 date-time and return the instant it denotes.
///
/// This preserves the accepted grammar used before lpcli moved from `chrono`
/// to `jiff`: `YYYY-MM-DD`, `T`/`t` or one space, `HH:MM:SS`, optional
/// fractional seconds, then `Z`/`z` or a `+HH:MM`/`-HH:MM` offset. It rejects
/// Temporal and ISO 8601 extensions such as shortened offsets, omitted seconds,
/// and bracketed time-zone annotations.
///
/// RFC 3339 permits `:60` for a positive leap second. As in the previous
/// implementation, that is represented as one nanosecond before the following
/// second, because `jiff` does not represent leap seconds.
pub fn parse_rfc3339_timestamp(value: &str) -> Option<Timestamp> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let digit_pair = |start: usize| -> Option<u8> {
        let tens = bytes.get(start)?.checked_sub(b'0').filter(|d| *d <= 9)?;
        let ones = bytes
            .get(start + 1)?
            .checked_sub(b'0')
            .filter(|d| *d <= 9)?;
        Some(tens * 10 + ones)
    };
    if !bytes[..4].iter().all(u8::is_ascii_digit)
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }

    let year = value[..4].parse::<i16>().ok()?;
    let month = digit_pair(5)?;
    let day = digit_pair(8)?;
    let hour = digit_pair(11)?;
    let minute = digit_pair(14)?;
    let second = digit_pair(17)?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut index = 19;
    let mut nanos = 0_i32;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            if index - start < 9 {
                nanos = nanos * 10 + i32::from(bytes[index] - b'0');
            }
            index += 1;
        }
        if index == start {
            return None;
        }
        let digits = (index - start).min(9);
        nanos *= 10_i32.pow(9 - u32::try_from(digits).ok()?);
    }

    let offset_seconds = match bytes.get(index)? {
        b'Z' | b'z' if index + 1 == bytes.len() => 0,
        sign @ (b'+' | b'-') if index + 6 == bytes.len() && bytes[index + 3] == b':' => {
            let offset_hour = digit_pair(index + 1)?;
            let offset_minute = digit_pair(index + 4)?;
            if offset_hour > 23 || offset_minute > 59 {
                return None;
            }
            let seconds = i32::from(offset_hour) * 3600 + i32::from(offset_minute) * 60;
            if *sign == b'-' { -seconds } else { seconds }
        }
        _ => return None,
    };

    let leap_second = second == 60;
    let datetime = jiff::civil::DateTime::new(
        year,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
        i8::try_from(hour).ok()?,
        i8::try_from(minute).ok()?,
        if leap_second {
            59
        } else {
            i8::try_from(second).ok()?
        },
        if leap_second { 999_999_999 } else { nanos },
    )
    .ok()?;
    Offset::from_seconds(offset_seconds)
        .ok()?
        .to_timestamp(datetime)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_rfc3339_accepts_previous_grammar() {
        for (input, expected) in [
            ("2026-11-17T12:34:56Z", "2026-11-17T12:34:56Z"),
            ("2026-11-17t12:34:56z", "2026-11-17T12:34:56Z"),
            ("2026-11-17 14:34:56+02:00", "2026-11-17T12:34:56Z"),
            (
                "2026-11-17T12:34:56.1234567899-01:30",
                "2026-11-17T14:04:56.123456789Z",
            ),
            ("2016-12-31T23:59:60Z", "2016-12-31T23:59:59.999999999Z"),
        ] {
            assert_eq!(
                parse_rfc3339_timestamp(input).unwrap().to_string(),
                expected,
                "{input}"
            );
        }
    }

    #[test]
    fn strict_rfc3339_rejects_temporal_and_iso_extensions() {
        for input in [
            "2024-03-10T02:05-04",
            "2024-03-10T02:05:00-04",
            "2024-03-10T02:05:00-0400",
            "2024-03-10T02:05:00-04:00[America/New_York]",
            "2024-03-10T02:05:00Z[UTC]",
            "2024-03-10T02:05:00.Z",
            "2024-03-10T02:05:00+24:00",
            "2024-03-10T02:05:00+23:60",
            "2024-02-30T02:05:00Z",
            "2024-03-10T24:00:00Z",
            "2024-03-10T23:59:61Z",
            "+2024-03-10T02:05:00Z",
            " 2024-03-10T02:05:00Z",
            "2024-03-10T02:05:00Z ",
        ] {
            assert!(parse_rfc3339_timestamp(input).is_none(), "{input}");
        }
    }
}
