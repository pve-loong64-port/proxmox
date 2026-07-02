//! A small, domain-free [RFC 5545] (iCalendar) serializer.
//!
//! Build [`Event`]s and a [`Calendar`], then serialize to a spec-correct `VCALENDAR` string
//! through [`Display`]. The crate owns only the wire format that is easy to get wrong - CRLF line
//! endings, 75-octet line folding on UTF-8 boundaries, `TEXT` escaping, `DATE` versus `DATE-TIME`
//! stamps, and DST-correct wall-clock-to-UTC conversion - and carries no calendar-application
//! logic, storage, or localization; the caller supplies every event and calendar field.
//!
//! # Examples
//!
//! ```
//! use proxmox_icalendar::{Calendar, Date, Event, EventStatus, EventTime};
//!
//! // A fixed-time event (UTC epoch seconds) and a whole-day event.
//! let meeting = Event::timed(
//!     "evt-1",
//!     "Team standup",
//!     EventTime::Utc(1_767_258_000),
//!     EventTime::Utc(1_767_259_800),
//! )
//! .with_status(EventStatus::Confirmed);
//! let holiday = Event::all_day("evt-2", "New Year", Date::constant(2026, 1, 1));
//!
//! let ics = Calendar::new("-//Proxmox//iCalendar//EN")
//!     .with_name("Schedule")
//!     .with_uid_domain("pve.example.com")
//!     .with_events(vec![meeting, holiday])
//!     .to_string();
//!
//! assert!(ics.starts_with("BEGIN:VCALENDAR\r\n"));
//! assert!(ics.contains("UID:evt-1@pve.example.com\r\n"));
//! ```
//!
//! Wall-clock times are given in local time plus an IANA zone and converted DST-correctly to UTC:
//!
//! ```
//! use proxmox_icalendar::{Calendar, Date, Event, EventTime};
//!
//! // 08:00 local on a Berlin summer day (CEST, UTC+2) becomes 06:00 UTC.
//! let shift = Event::timed(
//!     "evt-3",
//!     "Morning shift",
//!     EventTime::Local {
//!         date: Date::constant(2026, 7, 15),
//!         minute_of_day: 8 * 60,
//!         tz: "Europe/Berlin".into(),
//!     },
//!     EventTime::Local {
//!         date: Date::constant(2026, 7, 15),
//!         minute_of_day: 16 * 60,
//!         tz: "Europe/Berlin".into(),
//!     },
//! );
//!
//! let ics = Calendar::new("-//Proxmox//iCalendar//EN")
//!     .with_events(vec![shift])
//!     .to_string();
//! assert!(ics.contains("DTSTART:20260715T060000Z\r\n"));
//! ```
//!
//! # Guarantees
//!
//! - **Infallible.** `to_string()` always yields a valid calendar. A wall-clock
//!   [`EventTime::Local`] with no representable UTC instant (only at the civil-calendar extremes
//!   jiff can represent) falls back to the Unix epoch rather than failing.
//! - **Empty calendars serialize.** A calendar with no events emits an empty `VCALENDAR`; every
//!   client tolerates that even though RFC 5545 requires at least one component, so it is accepted
//!   by design. A caller that needs strict validity must guarantee at least one event itself.
//!
//! # Standards
//!
//! Output follows [RFC 5545] (iCalendar core) plus the [RFC 7986] `NAME` and
//! `DESCRIPTION` calendar properties.
//!
//! [RFC 5545]: https://www.rfc-editor.org/rfc/rfc5545
//! [RFC 7986]: https://www.rfc-editor.org/rfc/rfc7986
//! [`Display`]: std::fmt::Display

use std::fmt;

/// Re-exported civil [`Date`](jiff::civil::Date) used to build all-day and wall-clock event times
/// without a direct jiff dependency in the caller.
pub use jiff::civil::Date;

/// Maximum octets in one physical content line before folding (RFC 5545 3.1).
const MAX_LINE_OCTETS: usize = 75;

/// Escape a value for a `TEXT`-typed property (RFC 5545 3.3.11): a literal backslash, semicolon,
/// comma, or newline each gains a backslash prefix.
///
/// Any carriage return is first normalized to a line feed (a CRLF pair collapses to one LF), so a
/// value authored with CRLF line endings cannot smuggle a bare CR into the output; the surviving
/// newlines then escape to the two-character `\n`.
pub(crate) fn escape_ics_text(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    text.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
}

/// Format an epoch-seconds instant as a UTC `DATE-TIME` (`YYYYMMDDTHHMMSSZ`, RFC 5545 3.3.5).
///
/// An out-of-range epoch falls back to the Unix epoch instead of failing, so the serializer stays
/// infallible.
pub(crate) fn epoch_to_ics_utc(epoch: i64) -> String {
    let dt = match jiff::Timestamp::from_second(epoch) {
        Ok(ts) => ts.to_zoned(jiff::tz::TimeZone::UTC).datetime(),
        Err(_) => return "19700101T000000Z".to_string(),
    };
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    )
}

/// Format a civil date as an iCalendar `DATE` value (`YYYYMMDD`, RFC 5545 3.3.4).
fn format_ics_date(date: Date) -> String {
    format!("{:04}{:02}{:02}", date.year(), date.month(), date.day())
}

/// Failure converting a civil wall-clock time into a UTC iCalendar stamp.
#[derive(Debug, Clone)]
pub(crate) enum TimeError {
    /// The IANA time-zone identifier could not be loaded.
    UnknownTimezone(String),
    /// The civil instant has no representable UTC mapping. jiff resolves DST gaps and folds on its
    /// own, so in practice this only fires when the end-of-day rollover or the zoned timestamp
    /// overflows jiff's supported civil-calendar range.
    Unresolvable(String),
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimeError::UnknownTimezone(detail) => write!(f, "unknown time zone ({detail})"),
            TimeError::Unresolvable(detail) => write!(f, "unresolvable civil time ({detail})"),
        }
    }
}

impl std::error::Error for TimeError {}

/// Convert a civil `date` plus a `minute_of_day` wall-clock value in `tz` into a UTC `DATE-TIME`
/// stamp, handling two RFC 5545 corner cases:
///
/// - The minute is a *local* wall-clock value, so it maps to a true UTC instant DST-correctly
///   (RFC 5545 3.2/3.6.5) rather than stamping the local digits with a bare `Z`.
/// - Minute `1440` is `24:00`, an inclusive end-of-day that is illegal as hour 24 (RFC 5545
///   3.3.12); it rolls to `00:00` of the next civil day - the same instant - preserving the
///   duration.
///
/// `minute_of_day` is clamped to `0..=1440`; an out-of-range value is silently accepted at the
/// clamp boundary. `tz` is any IANA identifier (for example `Europe/Berlin`).
pub(crate) fn local_to_ics_utc(
    date: Date,
    minute_of_day: u16,
    tz: &str,
) -> Result<String, TimeError> {
    let minute_of_day = minute_of_day.min(1440);
    let civil = if minute_of_day == 1440 {
        date.tomorrow()
            .map_err(|e| TimeError::Unresolvable(format!("day after {date}: {e}")))?
            .at(0, 0, 0, 0)
    } else {
        date.at((minute_of_day / 60) as i8, (minute_of_day % 60) as i8, 0, 0)
    };
    let tz = jiff::tz::TimeZone::get(tz)
        .map_err(|e| TimeError::UnknownTimezone(format!("{tz}: {e}")))?;
    let ts = civil
        .to_zoned(tz)
        .map_err(|e| TimeError::Unresolvable(format!("{date} {minute_of_day}min: {e}")))?
        .timestamp();
    Ok(epoch_to_ics_utc(ts.as_second()))
}

/// `STATUS` value for a `VEVENT` (RFC 5545 3.8.1.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStatus {
    /// `TENTATIVE`: the event is provisional.
    Tentative,
    /// `CONFIRMED`: the event is definite.
    Confirmed,
    /// `CANCELLED`: the event was cancelled.
    Cancelled,
}

impl EventStatus {
    /// The RFC 5545 keyword for this status.
    fn as_ics(self) -> &'static str {
        match self {
            EventStatus::Tentative => "TENTATIVE",
            EventStatus::Confirmed => "CONFIRMED",
            EventStatus::Cancelled => "CANCELLED",
        }
    }
}

/// A `VEVENT` `ORGANIZER` (RFC 5545 3.8.4.3): the calendar user responsible for the event. Carries
/// a display `CN` and, when known, a `mailto:` address; without an address the value falls back to
/// the non-routable `invalid:nomail` URI so the property stays spec-valid.
#[derive(Debug, Clone)]
struct Organizer {
    cn: String,
    email: Option<String>,
}

/// Format a property-parameter value (RFC 5545 3.1/3.2): wrap it in `DQUOTE` when it carries a
/// COLON, SEMICOLON, or COMMA, which otherwise delimit the parameter list. A quoted string cannot
/// itself contain a `DQUOTE`, and a raw CR or LF would break line framing, so all three are dropped.
fn format_param_value(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .filter(|c| *c != '"' && *c != '\r' && *c != '\n')
        .collect();
    if sanitized.contains([':', ';', ',']) {
        format!("\"{sanitized}\"")
    } else {
        sanitized
    }
}

/// `TRANSP` value (RFC 5545 3.8.2.7): whether the event consumes the subscriber's free/busy time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transp {
    /// Blocks the time as busy. The RFC default, so it is omitted on the wire.
    #[default]
    Opaque,
    /// Informational only; does not mark the subscriber busy. Emits `TRANSP:TRANSPARENT`.
    Transparent,
}

/// A typed start or end value for an [`Event`]; the serializer renders each variant to its correct
/// iCalendar form.
#[derive(Debug, Clone)]
pub enum EventTime {
    /// A whole civil day. `DTSTART`/`DTEND` emit as `VALUE=DATE`, and an all-day `DTEND` is made
    /// the exclusive day after this date by the serializer.
    AllDay(Date),
    /// A fixed UTC instant in epoch seconds, emitted as a `DATE-TIME` with a trailing `Z`.
    Utc(i64),
    /// A wall-clock time: `minute_of_day` minutes past midnight on `date` in the IANA zone `tz`,
    /// converted to a true UTC `DATE-TIME`.
    Local {
        /// Civil date the wall-clock minute is measured on.
        date: Date,
        /// Minutes past local midnight, `0..=1440` (1440 is end-of-day 24:00).
        minute_of_day: u16,
        /// IANA time-zone identifier, for example `Europe/Berlin`.
        tz: String,
    },
}

impl EventTime {
    /// Whether this value is a `DATE` (all-day) rather than a `DATE-TIME`.
    fn is_all_day(&self) -> bool {
        matches!(self, EventTime::AllDay(_))
    }

    /// The `;VALUE=DATE` property parameter for an all-day value, empty for a `DATE-TIME`. Picked
    /// per value so `DTSTART` and `DTEND` each describe their own type rather than both following
    /// the start's.
    fn value_param(&self) -> &'static str {
        if self.is_all_day() { ";VALUE=DATE" } else { "" }
    }

    /// Format as a `DTSTART` value.
    fn format_start(&self) -> String {
        match self {
            EventTime::AllDay(date) => format_ics_date(*date),
            EventTime::Utc(epoch) => epoch_to_ics_utc(*epoch),
            EventTime::Local {
                date,
                minute_of_day,
                tz,
            } => {
                local_to_ics_utc(*date, *minute_of_day, tz).unwrap_or_else(|_| epoch_to_ics_utc(0))
            }
        }
    }

    /// Format as a `DTEND` value. An all-day end is exclusive, so it emits the civil day after the
    /// stored date (RFC 5545 3.6.1).
    fn format_end(&self) -> String {
        match self {
            EventTime::AllDay(date) => format_ics_date(date.tomorrow().unwrap_or(*date)),
            _ => self.format_start(),
        }
    }
}

/// A single `VEVENT`.
///
/// Build one with [`Event::all_day`], [`Event::all_day_range`], or [`Event::timed`], then layer
/// optional fields with the `with_*` methods. The type is `#[non_exhaustive]` and has no public
/// fields, so it can only be constructed through those builders.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Event {
    uid: String,
    summary: String,
    start: EventTime,
    end: EventTime,
    description: Option<String>,
    status: Option<EventStatus>,
    transp: Transp,
    organizer: Option<Organizer>,
    dtstamp: Option<i64>,
    last_modified: Option<i64>,
}

impl Event {
    fn base(
        uid: impl Into<String>,
        summary: impl Into<String>,
        start: EventTime,
        end: EventTime,
    ) -> Self {
        Event {
            uid: uid.into(),
            summary: summary.into(),
            start,
            end,
            description: None,
            status: None,
            transp: Transp::Opaque,
            organizer: None,
            dtstamp: None,
            last_modified: None,
        }
    }

    /// A whole-day event on a single civil `date`. `DTEND` emits the exclusive next day (RFC 5545
    /// 3.6.1).
    pub fn all_day(uid: impl Into<String>, summary: impl Into<String>, date: Date) -> Self {
        Self::base(
            uid,
            summary,
            EventTime::AllDay(date),
            EventTime::AllDay(date),
        )
    }

    /// A whole-day event spanning `start..=end` inclusive. `DTEND` emits the exclusive day after
    /// `end`.
    pub fn all_day_range(
        uid: impl Into<String>,
        summary: impl Into<String>,
        start: Date,
        end: Date,
    ) -> Self {
        Self::base(
            uid,
            summary,
            EventTime::AllDay(start),
            EventTime::AllDay(end),
        )
    }

    /// An event with typed `start` and `end` instants (UTC or local wall-clock). Use
    /// [`all_day`](Self::all_day) / [`all_day_range`](Self::all_day_range) for `DATE` values.
    pub fn timed(
        uid: impl Into<String>,
        summary: impl Into<String>,
        start: EventTime,
        end: EventTime,
    ) -> Self {
        Self::base(uid, summary, start, end)
    }

    /// Set the `DESCRIPTION` text (escaped on emit).
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the `STATUS`.
    pub fn with_status(mut self, status: EventStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Set the `TRANSP`. `Transparent` keeps the event from consuming the subscriber's free/busy
    /// time; `Opaque` is the default and is omitted.
    pub fn with_transp(mut self, transp: Transp) -> Self {
        self.transp = transp;
        self
    }

    /// Set the `ORGANIZER` (RFC 5545 3.8.4.3). `cn` is the display name; `email`, when given, is
    /// emitted as the `mailto:` calendar address, otherwise the value falls back to a non-routable
    /// `invalid:nomail` URI.
    pub fn with_organizer(mut self, cn: impl Into<String>, email: Option<String>) -> Self {
        self.organizer = Some(Organizer {
            cn: cn.into(),
            email,
        });
        self
    }

    /// Set `LAST-MODIFIED` (epoch seconds). This also becomes the default `DTSTAMP` unless
    /// [`with_dtstamp`](Self::with_dtstamp) overrides it.
    pub fn with_last_modified(mut self, epoch: i64) -> Self {
        self.last_modified = Some(epoch);
        self
    }

    /// Override `DTSTAMP` (epoch seconds) independently of `LAST-MODIFIED`. `DTSTAMP` is REQUIRED,
    /// so it always emits; without this it mirrors `LAST-MODIFIED`, falling back to the Unix epoch
    /// when neither is set.
    pub fn with_dtstamp(mut self, epoch: i64) -> Self {
        self.dtstamp = Some(epoch);
        self
    }
}

/// A `VCALENDAR` published with `METHOD:PUBLISH` on the Gregorian scale.
///
/// Build one with [`Calendar::new`] and the `with_*` methods, then serialize through
/// [`Display`](std::fmt::Display) / `to_string()`. The type is `#[non_exhaustive]`.
///
/// A calendar with no events still serializes (to an empty `VCALENDAR`); see the crate-level note.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Calendar {
    prodid: String,
    uid_domain: Option<String>,
    name: Option<String>,
    description: Option<String>,
    timezone: Option<String>,
    events: Vec<Event>,
}

impl Calendar {
    /// Create a calendar with the required `PRODID` (RFC 5545 3.7.3).
    pub fn new(prodid: impl Into<String>) -> Self {
        Calendar {
            prodid: prodid.into(),
            uid_domain: None,
            name: None,
            description: None,
            timezone: None,
            events: Vec::new(),
        }
    }

    /// Append `@domain` to every event UID. Without it each [`Event`] UID emits verbatim.
    pub fn with_uid_domain(mut self, domain: impl Into<String>) -> Self {
        self.uid_domain = Some(domain.into());
        self
    }

    /// Set the calendar name, emitted as `X-WR-CALNAME` plus the RFC 7986 `NAME`.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the calendar description, emitted as `X-WR-CALDESC` plus the RFC 7986 `DESCRIPTION`.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the `X-WR-TIMEZONE` display-zone hint.
    pub fn with_timezone(mut self, timezone: impl Into<String>) -> Self {
        self.timezone = Some(timezone.into());
        self
    }

    /// Set the event list. The caller owns ordering; events emit as given.
    pub fn with_events(mut self, events: Vec<Event>) -> Self {
        self.events = events;
        self
    }

    fn write_event<W: fmt::Write>(&self, out: &mut W, ev: &Event) -> fmt::Result {
        let uid = match &self.uid_domain {
            Some(domain) => format!("{}@{}", ev.uid, domain),
            None => ev.uid.clone(),
        };
        // DTSTAMP is REQUIRED; mirror LAST-MODIFIED when not set explicitly.
        let dtstamp = ev.dtstamp.or(ev.last_modified).unwrap_or(0);
        write_content_line(out, "BEGIN:VEVENT")?;
        write_content_line(out, &format!("UID:{}", escape_ics_text(&uid)))?;
        write_content_line(out, &format!("DTSTAMP:{}", epoch_to_ics_utc(dtstamp)))?;
        write_content_line(out, &format!("SUMMARY:{}", escape_ics_text(&ev.summary)))?;
        write_content_line(
            out,
            &format!(
                "DTSTART{}:{}",
                ev.start.value_param(),
                ev.start.format_start()
            ),
        )?;
        write_content_line(
            out,
            &format!("DTEND{}:{}", ev.end.value_param(), ev.end.format_end()),
        )?;
        if let Some(desc) = &ev.description {
            write_content_line(out, &format!("DESCRIPTION:{}", escape_ics_text(desc)))?;
        }
        if let Some(status) = ev.status {
            write_content_line(out, &format!("STATUS:{}", status.as_ics()))?;
        }
        if let Some(org) = &ev.organizer {
            let addr = match &org.email {
                Some(mail) => format!("mailto:{mail}"),
                None => "invalid:nomail".to_string(),
            };
            write_content_line(
                out,
                &format!("ORGANIZER;CN={}:{}", format_param_value(&org.cn), addr),
            )?;
        }
        if ev.transp == Transp::Transparent {
            write_content_line(out, "TRANSP:TRANSPARENT")?;
        }
        if let Some(epoch) = ev.last_modified {
            write_content_line(out, &format!("LAST-MODIFIED:{}", epoch_to_ics_utc(epoch)))?;
        }
        write_content_line(out, "END:VEVENT")
    }
}

/// Append one already-escaped content `line` to `out`, folded to <=75 octets (RFC 5545 3.1) and
/// terminated with CRLF.
///
/// Folding counts UTF-8 octets but only ever splits on a char boundary, so a multibyte character is
/// never cut; each continuation line starts with a single space that counts toward its 75-octet
/// budget.
fn write_content_line<W: fmt::Write>(out: &mut W, line: &str) -> fmt::Result {
    if line.is_empty() {
        return out.write_str("\r\n");
    }
    let mut start = 0;
    let mut first = true;
    while start < line.len() {
        // The leading space on a continuation line eats one octet of budget.
        let budget = if first {
            MAX_LINE_OCTETS
        } else {
            MAX_LINE_OCTETS - 1
        };
        let mut end = (start + budget).min(line.len());
        while end < line.len() && !line.is_char_boundary(end) {
            end -= 1;
        }
        if !first {
            out.write_char(' ')?;
        }
        out.write_str(&line[start..end])?;
        out.write_str("\r\n")?;
        start = end;
        first = false;
    }
    Ok(())
}

impl fmt::Display for Calendar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_content_line(f, "BEGIN:VCALENDAR")?;
        write_content_line(f, "VERSION:2.0")?;
        write_content_line(f, &format!("PRODID:{}", escape_ics_text(&self.prodid)))?;
        write_content_line(f, "CALSCALE:GREGORIAN")?;
        write_content_line(f, "METHOD:PUBLISH")?;
        if let Some(name) = &self.name {
            // X-WR-CALNAME is the de-facto client property; NAME is its standardized RFC 7986
            // equivalent, emitted alongside for symmetry.
            write_content_line(f, &format!("X-WR-CALNAME:{}", escape_ics_text(name)))?;
            write_content_line(f, &format!("NAME:{}", escape_ics_text(name)))?;
        }
        if let Some(desc) = &self.description {
            // Likewise X-WR-CALDESC plus the RFC 7986 calendar DESCRIPTION.
            write_content_line(f, &format!("X-WR-CALDESC:{}", escape_ics_text(desc)))?;
            write_content_line(f, &format!("DESCRIPTION:{}", escape_ics_text(desc)))?;
        }
        if let Some(tz) = &self.timezone {
            write_content_line(f, &format!("X-WR-TIMEZONE:{}", escape_ics_text(tz)))?;
        }
        for ev in &self.events {
            self.write_event(f, ev)?;
        }
        write_content_line(f, "END:VCALENDAR")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn sample_event() -> Event {
        // 2026-01-01 09:00-09:30 UTC; DTSTAMP / LAST-MODIFIED at 08:00 UTC.
        Event::timed(
            "evt-1",
            "Standup",
            EventTime::Utc(1_767_258_000),
            EventTime::Utc(1_767_259_800),
        )
        .with_status(EventStatus::Confirmed)
        .with_last_modified(1_767_254_400)
    }

    fn sample_calendar(events: Vec<Event>) -> Calendar {
        Calendar::new("-//Example//Test//EN")
            .with_uid_domain("example.com")
            .with_name("Test")
            .with_description("A test calendar")
            .with_timezone("Europe/Berlin")
            .with_events(events)
    }

    /// Assert `ics` uses only CRLF line breaks: every CR is followed by an LF and every LF is
    /// preceded by a CR, so no bare CR or LF survives.
    fn assert_crlf_framing(ics: &str) {
        let bytes = ics.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\r' {
                assert!(bytes.get(i + 1) == Some(&b'\n'), "bare CR at offset {i}");
            }
            if b == b'\n' {
                assert!(i > 0 && bytes[i - 1] == b'\r', "bare LF at offset {i}");
            }
        }
    }

    #[test]
    fn escape_ics_text_escapes_special_chars() {
        // Backslash is escaped first so the prefixes added for `;`, `,`, and a newline are not
        // themselves doubled.
        assert_eq!(escape_ics_text("a;b,c\\d\ne"), "a\\;b\\,c\\\\d\\ne");
    }

    #[test]
    fn escape_ics_text_normalizes_carriage_returns() {
        // A CRLF-authored value must not smuggle a bare CR into the output: the CRLF collapses
        // to one LF, which escapes to the two-character `\n`. A lone CR is treated the same way.
        assert_eq!(escape_ics_text("a\r\nb"), "a\\nb");
        assert_eq!(escape_ics_text("a\rb"), "a\\nb");
        assert!(!escape_ics_text("a\r\nb").contains('\r'));
    }

    #[test]
    fn crlf_description_serializes_without_raw_cr() {
        // A CRLF-laden DESCRIPTION escapes to `\n` and leaves no bare CR in the rendered calendar.
        let ev = Event::timed(
            "e",
            "S",
            EventTime::Utc(1_767_258_000),
            EventTime::Utc(1_767_259_800),
        )
        .with_description("line1\r\nline2")
        .with_last_modified(1_767_254_400);
        let ics = sample_calendar(vec![ev]).to_string();
        assert!(ics.contains("DESCRIPTION:line1\\nline2"));
        assert_crlf_framing(&ics);
    }

    #[test]
    fn format_ics_date_drops_separators() {
        assert_eq!(format_ics_date(date(2026, 12, 25)), "20261225");
    }

    #[test]
    fn epoch_to_ics_utc_formats_utc_datetime() {
        // 1_700_000_000 is 2023-11-14T22:13:20Z.
        assert_eq!(epoch_to_ics_utc(1_700_000_000), "20231114T221320Z");
    }

    #[test]
    fn write_content_line_folds_at_75_octets() {
        // RFC 5545 3.1: no physical line exceeds 75 octets, and unfolding (drop CRLF plus the
        // leading space) restores the original line.
        let line = format!("SUMMARY:{}", "x".repeat(200));
        let mut out = String::new();
        write_content_line(&mut out, &line).unwrap();
        for phys in out.split("\r\n").filter(|s| !s.is_empty()) {
            assert!(
                phys.len() <= 75,
                "physical line {} octets: {phys}",
                phys.len()
            );
        }
        assert!(out.ends_with("\r\n"));
        let unfolded = out.trim_end_matches("\r\n").replace("\r\n ", "");
        assert_eq!(unfolded, line);
    }

    #[test]
    fn write_content_line_never_splits_multibyte_char() {
        // 3-octet euro signs packed so a naive 75-byte cut would land mid-char; slicing at a
        // non-boundary would panic.
        let line = format!("SUMMARY:{}", "\u{20AC}".repeat(60));
        let mut out = String::new();
        write_content_line(&mut out, &line).unwrap();
        for phys in out.split("\r\n").filter(|s| !s.is_empty()) {
            assert!(phys.len() <= 75, "physical line {} octets", phys.len());
        }
        let unfolded = out.trim_end_matches("\r\n").replace("\r\n ", "");
        assert_eq!(unfolded, line);
    }

    #[test]
    fn calendar_emits_crlf_endings_only() {
        // RFC 5545 3.1: every content line ends with CRLF; no bare CR or LF survives.
        let ics = sample_calendar(vec![sample_event()]).to_string();
        assert_crlf_framing(&ics);
        assert!(ics.ends_with("\r\n"));
    }

    #[test]
    fn every_vevent_carries_one_dtstamp() {
        // RFC 5545 3.6.1: DTSTAMP is REQUIRED in each VEVENT.
        let ics = sample_calendar(vec![sample_event(), sample_event()]).to_string();
        let vevents = ics.matches("BEGIN:VEVENT").count();
        let dtstamps = ics.matches("DTSTAMP:").count();
        assert_eq!(vevents, 2);
        assert_eq!(vevents, dtstamps, "every VEVENT needs exactly one DTSTAMP");
        assert!(ics.contains("DTSTAMP:20260101T080000Z"));
    }

    #[test]
    fn dtstamp_defaults_to_last_modified_and_can_be_overridden() {
        let ics = sample_calendar(vec![sample_event()]).to_string();
        assert!(ics.contains("DTSTAMP:20260101T080000Z"));
        assert!(ics.contains("LAST-MODIFIED:20260101T080000Z"));

        // 1_767_225_600 is 2026-01-01T00:00:00Z.
        let ev = sample_event().with_dtstamp(1_767_225_600);
        let ics = sample_calendar(vec![ev]).to_string();
        assert!(ics.contains("DTSTAMP:20260101T000000Z"));
        assert!(ics.contains("LAST-MODIFIED:20260101T080000Z"));
    }

    #[test]
    fn fold_round_trips_through_calendar_output() {
        // A SUMMARY past 75 octets is folded with CRLF + space and unfolds whole.
        let summary = "S".repeat(120);
        let ev = Event::timed(
            "evt-1",
            summary.clone(),
            EventTime::Utc(1_767_258_000),
            EventTime::Utc(1_767_259_800),
        )
        .with_last_modified(1_767_254_400);
        let ics = sample_calendar(vec![ev]).to_string();
        assert!(ics.contains("\r\n "), "long SUMMARY must be folded");
        let unfolded = ics.replace("\r\n ", "");
        assert!(unfolded.contains(&format!("SUMMARY:{summary}")));
    }

    #[test]
    fn uid_domain_is_appended_else_verbatim() {
        let with = sample_calendar(vec![sample_event()]).to_string();
        assert!(with.contains("UID:evt-1@example.com"));

        let without = Calendar::new("-//Example//Test//EN")
            .with_events(vec![sample_event()])
            .to_string();
        assert!(without.contains("UID:evt-1\r\n"));
    }

    #[test]
    fn all_day_range_uses_value_date_params() {
        let ev = Event::all_day_range("evt-1", "Holiday", date(2026, 1, 1), date(2026, 1, 1))
            .with_status(EventStatus::Confirmed)
            .with_last_modified(1_767_254_400);
        let ics = sample_calendar(vec![ev]).to_string();
        assert!(ics.contains("DTSTART;VALUE=DATE:20260101"));
        // DTEND is the exclusive next day.
        assert!(ics.contains("DTEND;VALUE=DATE:20260102"));
    }

    #[test]
    fn dtstart_and_dtend_select_value_param_independently() {
        // DTSTART and DTEND each tag VALUE=DATE from their own value, so a DATE start paired with a
        // DATE-TIME end never stamps VALUE=DATE onto the DATE-TIME line.
        let ev = Event::timed(
            "e",
            "S",
            EventTime::AllDay(date(2026, 1, 1)),
            EventTime::Utc(1_767_259_800),
        );
        let ics = sample_calendar(vec![ev]).to_string();
        assert!(ics.contains("DTSTART;VALUE=DATE:20260101\r\n"));
        assert!(ics.contains("DTEND:20260101T093000Z\r\n"));
    }

    #[test]
    fn transparent_emits_transp_line_opaque_omits_it() {
        let opaque = sample_calendar(vec![sample_event()]).to_string();
        assert!(
            !opaque.contains("TRANSP:"),
            "Opaque is the default and omitted"
        );

        let ev = sample_event().with_transp(Transp::Transparent);
        let transparent = sample_calendar(vec![ev]).to_string();
        assert!(transparent.contains("TRANSP:TRANSPARENT"));
    }

    #[test]
    fn status_emitted_only_when_set() {
        let ev = Event::timed(
            "e",
            "S",
            EventTime::Utc(1_767_258_000),
            EventTime::Utc(1_767_259_800),
        )
        .with_last_modified(1_767_254_400);
        let without = sample_calendar(vec![ev]).to_string();
        assert!(!without.contains("STATUS:"));

        let with = sample_calendar(vec![sample_event()]).to_string();
        assert!(with.contains("STATUS:CONFIRMED"));
    }

    #[test]
    fn last_modified_emitted_only_when_set() {
        let ev = Event::timed(
            "e",
            "S",
            EventTime::Utc(1_767_258_000),
            EventTime::Utc(1_767_259_800),
        );
        let ics = sample_calendar(vec![ev]).to_string();
        assert!(!ics.contains("LAST-MODIFIED:"));
        // DTSTAMP is REQUIRED, so it still appears (falling back to the epoch).
        assert!(ics.contains("DTSTAMP:19700101T000000Z"));
    }

    #[test]
    fn empty_calendar_renders_required_props_and_omits_optionals() {
        // Built with only a PRODID: required properties present, every optional field absent, and
        // no VEVENT (the documented empty-calendar case).
        let ics = Calendar::new("-//Example//Test//EN").to_string();
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(ics.contains("VERSION:2.0"));
        assert!(ics.ends_with("END:VCALENDAR\r\n"));
        assert!(!ics.contains("BEGIN:VEVENT"));
        assert!(!ics.contains("X-WR-CALNAME"));
        assert!(!ics.contains("X-WR-CALDESC"));
        assert!(!ics.contains("X-WR-TIMEZONE"));
        assert!(!ics.contains("\r\nNAME:"));
        assert!(!ics.contains("\r\nDESCRIPTION:"));
    }

    #[test]
    fn calendar_emits_rfc7986_name_and_description() {
        let ics = sample_calendar(vec![]).to_string();
        assert!(ics.contains("X-WR-CALNAME:Test"));
        assert!(ics.contains("\r\nNAME:Test\r\n"));
        assert!(ics.contains("X-WR-CALDESC:A test calendar"));
        assert!(ics.contains("DESCRIPTION:A test calendar"));
    }

    #[test]
    fn organizer_emits_cn_and_mailto_or_falls_back() {
        // With an address the ORGANIZER carries CN plus mailto; without one it falls back to the
        // non-routable invalid:nomail value rather than an empty CAL-ADDRESS.
        let with_mail =
            sample_event().with_organizer("Tina Lead", Some("tlead@example.com".into()));
        let ics = sample_calendar(vec![with_mail]).to_string();
        assert!(ics.contains("ORGANIZER;CN=Tina Lead:mailto:tlead@example.com"));

        let no_mail = sample_event().with_organizer("Tina Lead", None);
        let ics = sample_calendar(vec![no_mail]).to_string();
        assert!(ics.contains("ORGANIZER;CN=Tina Lead:invalid:nomail"));

        // A CN with a separator char is double-quoted so it cannot break the parameter list.
        let comma = sample_event().with_organizer("Lead, Tina", Some("t@example.com".into()));
        let ics = sample_calendar(vec![comma]).to_string();
        assert!(ics.contains("ORGANIZER;CN=\"Lead, Tina\":mailto:t@example.com"));

        // Omitted by default.
        let plain = sample_calendar(vec![sample_event()]).to_string();
        assert!(!plain.contains("ORGANIZER"));
    }

    #[test]
    fn local_applies_dst_offset() {
        // RFC 5545 3.2/3.6.5: a local minute becomes a true UTC instant. In a Central European zone
        // 08:00 (480 min) is 06:00Z in summer (UTC+2) and 07:00Z in winter (UTC+1).
        assert_eq!(
            local_to_ics_utc(date(2026, 7, 15), 480, "Europe/Berlin").unwrap(),
            "20260715T060000Z"
        );
        assert_eq!(
            local_to_ics_utc(date(2026, 1, 15), 480, "Europe/Berlin").unwrap(),
            "20260115T070000Z"
        );
    }

    #[test]
    fn local_midnight_never_emits_hour_24() {
        // RFC 5545 3.3.12: minute 1440 (24:00) rolls to 00:00 of the next civil day - the same
        // instant - which is 23:00Z (CET) or 22:00Z (CEST).
        let winter = local_to_ics_utc(date(2026, 1, 15), 1440, "Europe/Berlin").unwrap();
        let summer = local_to_ics_utc(date(2026, 7, 15), 1440, "Europe/Berlin").unwrap();
        assert_eq!(winter, "20260115T230000Z");
        assert_eq!(summer, "20260715T220000Z");
        assert!(
            !winter.contains("240000") && !summer.contains("240000"),
            "hour 24 must never be emitted"
        );
    }

    #[test]
    fn local_rejects_unknown_zone() {
        assert!(matches!(
            local_to_ics_utc(date(2026, 7, 15), 480, "Mars/Olympus"),
            Err(TimeError::UnknownTimezone(_))
        ));
    }

    #[test]
    fn local_event_time_renders_as_utc_datetime() {
        // An EventTime::Local start/end is converted DST-correctly by the serializer and emitted
        // as a bare DATE-TIME (no VALUE=DATE).
        let ev = Event::timed(
            "evt-1",
            "Half day",
            EventTime::Local {
                date: date(2026, 7, 15),
                minute_of_day: 480,
                tz: "Europe/Berlin".to_string(),
            },
            EventTime::Local {
                date: date(2026, 7, 15),
                minute_of_day: 720,
                tz: "Europe/Berlin".to_string(),
            },
        )
        .with_last_modified(1_767_254_400);
        let ics = sample_calendar(vec![ev]).to_string();
        assert!(ics.contains("DTSTART:20260715T060000Z"));
        assert!(ics.contains("DTEND:20260715T100000Z"));
        assert!(!ics.contains("VALUE=DATE"));
    }
}
