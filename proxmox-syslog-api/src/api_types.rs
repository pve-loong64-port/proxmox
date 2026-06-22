use serde::{Deserialize, Serialize};

use anyhow::{Error, bail};
use proxmox_schema::api_types::SYSTEMD_DATETIME_FORMAT;
use proxmox_schema::{ApiStringFormat, api};

// validate the priority filter: a single syslog level 0 to 7, or a LOW..HIGH range of them. Done
// with a verify function rather than a regex to avoid pulling the regex crate into this small crate
fn verify_journal_priority(value: &str) -> Result<(), Error> {
    let is_level = |p: &str| p.len() == 1 && matches!(p.as_bytes()[0], b'0'..=b'7');
    let valid = value.is_empty()
        || match value.split_once("..") {
            Some((low, high)) => is_level(low) && is_level(high),
            None => is_level(value),
        };
    if !valid {
        bail!("'{value}' is not a valid syslog priority, expected 0-7 or LOW..HIGH");
    }
    Ok(())
}

const JOURNAL_PRIORITY_FORMAT: ApiStringFormat = ApiStringFormat::VerifyFn(verify_journal_priority);

#[api(
    properties: {
        start: {
            type: Integer,
            description: "Start line number.",
            minimum: 0,
            optional: true,
        },
        limit: {
            type: Integer,
            description: "Max. number of lines.",
            optional: true,
            minimum: 0,
        },
        since: {
            type: String,
            optional: true,
            description: "Display all log since this date-time string.",
	        format: &SYSTEMD_DATETIME_FORMAT,
        },
        until: {
            type: String,
            optional: true,
            description: "Display all log until this date-time string.",
	        format: &SYSTEMD_DATETIME_FORMAT,
        },
        service: {
            type: String,
            optional: true,
            description: "Service ID.",
            max_length: 128,
        },
    },
)]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
/// Syslog API filtering options.
pub struct SyslogFilter {
    pub start: Option<u64>,
    pub limit: Option<u64>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub service: Option<String>,
}

#[api]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
/// Syslog line with line number.
pub struct SyslogLine {
    /// Line number.
    pub n: u64,
    /// Line text.
    pub t: String,
}

#[api(
    properties: {
        since: {
            type: Integer,
            optional: true,
            description: "Display all log since this UNIX epoch. Conflicts with 'startcursor'.",
            minimum: 0,
        },
        until: {
            type: Integer,
            optional: true,
            description: "Display all log until this UNIX epoch. Conflicts with 'endcursor'.",
            minimum: 0,
        },
        lastentries: {
            type: Integer,
            optional: true,
            description: "Limit to the last X lines. Conflicts with a range.",
            minimum: 0,
        },
        startcursor: {
            type: String,
            description: "Start after the given Cursor. Conflicts with 'since'.",
            optional: true,
        },
        endcursor: {
            type: String,
            description: "End before the given Cursor. Conflicts with 'until'",
            optional: true,
        },
        priority: {
            type: String,
            optional: true,
            format: &JOURNAL_PRIORITY_FORMAT,
            description: "Only print messages of this syslog priority: a single \
                level from 0 (emerg) to 7 (debug), selecting that level and \
                everything more severe, or a 'LOW..HIGH' range. Empty means no filter.",
        },
        structured: {
            type: Boolean,
            optional: true,
            default: false,
            description: "Emit structured JSON with separate entry fields instead of plain text.",
        },
        service: {
            type: String,
            optional: true,
            description: "Only print messages whose syslog identifier matches this glob.",
        },
        unit: {
            type: String,
            optional: true,
            description: "Only print messages of this systemd unit (the .service suffix is implied).",
        },
        kernel: {
            type: Boolean,
            optional: true,
            default: false,
            description: "Only print kernel messages.",
        },
        identifiers: {
            type: Boolean,
            optional: true,
            default: false,
            description: "Also list the distinct syslog identifiers present. Requires 'structured'.",
        },
        units: {
            type: Boolean,
            optional: true,
            default: false,
            description: "Also list the distinct systemd units present. Requires 'structured'.",
        },
    }
)]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
/// Journal API filtering options.
pub struct JournalFilter {
    pub since: Option<u64>,
    pub until: Option<u64>,
    pub lastentries: Option<u64>,
    pub startcursor: Option<String>,
    pub endcursor: Option<String>,
    pub priority: Option<String>,
    #[serde(default)]
    pub structured: bool,
    pub service: Option<String>,
    pub unit: Option<String>,
    #[serde(default)]
    pub kernel: bool,
    #[serde(default)]
    pub identifiers: bool,
    #[serde(default)]
    pub units: bool,
}
