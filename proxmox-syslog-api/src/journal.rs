use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use anyhow::Error;

use super::{JournalFilter, SyslogFilter, SyslogLine};

/// Syslog API implementation
///
/// The syslog api uses `journalctl' to get the log entries, and
/// uses paging to limit the amount of data returned (start, limit).
///
/// Note: Please use [dump_journal] for live view, because that is more performant
/// for that case.
pub fn dump_syslog(filter: SyslogFilter) -> Result<(u64, Vec<SyslogLine>), Error> {
    let mut args = vec!["-o", "short", "--no-pager"];

    if let Some(service) = &filter.service {
        args.extend(["--unit", service]);
    }
    if let Some(since) = &filter.since {
        args.extend(["--since", since]);
    }
    if let Some(until) = &filter.until {
        args.extend(["--until", until]);
    }

    let mut lines: Vec<SyslogLine> = Vec::new();
    let mut limit = filter.limit.unwrap_or(50);
    let start = filter.start.unwrap_or(0);
    let mut count: u64 = 0;

    let mut child = Command::new("journalctl")
        .args(&args)
        .stdout(Stdio::piped())
        .spawn()?;

    use std::io::{BufRead, BufReader};

    if let Some(ref mut stdout) = child.stdout {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    count += 1;
                    if count < start {
                        continue;
                    };
                    if limit == 0 {
                        continue;
                    };

                    lines.push(SyslogLine { n: count, t: line });

                    limit -= 1;
                }
                Err(err) => {
                    log::error!("reading journal failed: {}", err);
                    let _ = child.kill();
                    break;
                }
            }
        }
    }

    let status = child.wait().unwrap();
    if !status.success() {
        log::error!("journalctl failed with {}", status);
    }

    // HACK: ExtJS store.guaranteeRange() does not like empty array
    // so we add a line
    if count == 0 {
        count += 1;
        lines.push(SyslogLine {
            n: count,
            t: String::from("no content"),
        });
    }

    Ok((count, lines))
}

/// Journal API implementation
///
/// The journal api uses `mini-journalreader' binary to get the log entries.
/// The cursor based api allows to implement live view efficiently.
#[deprecated(note = "superseded by journal_args(); stream the reader's -j/-J output directly")]
pub fn dump_journal(filter: JournalFilter) -> Result<Vec<String>, Error> {
    let mut args = vec![];

    if let Some(lastentries) = filter.lastentries {
        args.push(String::from("-n"));
        args.push(format!("{lastentries}"));
    }

    if let Some(since) = filter.since {
        args.push(String::from("-b"));
        args.push(since.to_string());
    }

    if let Some(until) = filter.until {
        args.push(String::from("-e"));
        args.push(until.to_string());
    }

    if let Some(startcursor) = &filter.startcursor {
        args.push(String::from("-f"));
        args.push(startcursor.to_string());
    }

    if let Some(endcursor) = &filter.endcursor {
        args.push(String::from("-t"));
        args.push(endcursor.to_string());
    }

    if let Some(priority) = &filter.priority {
        args.push(String::from("-p"));
        args.push(priority.to_string());
    }

    let mut lines: Vec<String> = vec![];

    let mut child = Command::new("mini-journalreader")
        .args(&args)
        .stdout(Stdio::piped())
        .spawn()?;

    if let Some(ref mut stdout) = child.stdout {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    lines.push(line);
                }
                Err(err) => {
                    log::error!("reading journal failed: {}", err);
                    let _ = child.kill();
                    break;
                }
            }
        }
    }

    let status = child.wait().unwrap();
    if !status.success() {
        log::error!("journalctl failed with {}", status);
    }

    Ok(lines)
}

/// Build the `mini-journalreader` arguments for the given journal filter.
///
/// Selects `-J` (structured records) or `-j` (plain lines); both already emit the
/// `{"data":[...],"success":1}` API envelope, so a caller can run the reader and stream its stdout
/// straight through as the response body instead of collecting and re-encoding it.
///
/// The structured output and the priority, unit, identifier and kernel filters need
/// proxmox-mini-journalreader >= 1.7, which introduced the corresponding reader flags.
pub fn journal_args(filter: &JournalFilter) -> Vec<String> {
    let mut args = vec![if filter.structured { "-J" } else { "-j" }.to_string()];

    if let Some(lastentries) = filter.lastentries {
        args.push("-n".to_string());
        args.push(lastentries.to_string());
    }
    if let Some(since) = filter.since {
        args.push("-b".to_string());
        args.push(since.to_string());
    }
    if let Some(until) = filter.until {
        args.push("-e".to_string());
        args.push(until.to_string());
    }
    if let Some(startcursor) = &filter.startcursor {
        args.push("-f".to_string());
        args.push(startcursor.clone());
    }
    if let Some(endcursor) = &filter.endcursor {
        args.push("-t".to_string());
        args.push(endcursor.clone());
    }
    if let Some(priority) = &filter.priority {
        if !priority.is_empty() {
            args.push("-p".to_string());
            args.push(priority.clone());
        }
    }
    if let Some(service) = &filter.service {
        args.push("-i".to_string());
        args.push(service.clone());
    }
    if let Some(unit) = &filter.unit {
        // a few service names differ from the unit that actually logs
        let unit = match unit.as_str() {
            "postfix" => "postfix@-",
            "sshd" => "ssh",
            other => other,
        };
        args.push("-u".to_string());
        args.push(unit.to_string());
    }
    if filter.kernel {
        args.push("-k".to_string());
    }
    if filter.structured && filter.identifiers {
        args.push("-I".to_string());
    }
    if filter.structured && filter.units {
        args.push("-U".to_string());
    }

    args
}
