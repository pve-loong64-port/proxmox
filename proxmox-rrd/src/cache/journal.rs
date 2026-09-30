use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufReader, Write};
use std::os::unix::fs::FileExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Error, bail, format_err};
use crossbeam_channel::Receiver;
use nix::fcntl::{FcntlArg, OFlag, fcntl};

use proxmox_sys::fs::atomic_open_or_create_file;

const RRD_JOURNAL_NAME: &str = "rrd.journal";

use crate::cache::CacheConfig;
use crate::rrd::DataSourceType;

// shared state behind RwLock
pub struct JournalState {
    config: Arc<CacheConfig>,
    journal: File,
    pub last_journal_flush: f64,
    pub journal_applied: bool,
    pub apply_thread_result: Option<Receiver<Result<(), String>>>,
}

pub struct JournalEntry {
    pub time: f64,
    pub value: f64,
    pub dst: DataSourceType,
    pub rel_path: String,
}

impl FromStr for JournalEntry {
    type Err = Error;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let line = line.trim();

        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() != 4 {
            bail!("wrong number of components");
        }

        let time: f64 = parts[0]
            .parse()
            .map_err(|_| format_err!("unable to parse time"))?;
        let value: f64 = parts[1]
            .parse()
            .map_err(|_| format_err!("unable to parse value"))?;
        let dst: u8 = parts[2]
            .parse()
            .map_err(|_| format_err!("unable to parse data source type"))?;

        let dst = match dst {
            0 => DataSourceType::Gauge,
            1 => DataSourceType::Derive,
            _ => bail!("got strange value for data source type '{}'", dst),
        };

        let rel_path = parts[3].to_string();

        Ok(JournalEntry {
            time,
            value,
            dst,
            rel_path,
        })
    }
}

pub struct JournalFileInfo {
    pub time: u64,
    pub name: String,
    pub path: PathBuf,
}

impl JournalState {
    pub(crate) fn new(config: Arc<CacheConfig>) -> Result<Self, Error> {
        let journal = JournalState::open_journal_writer(&config)?;
        if let Err(err) = truncate_incomplete_entry(&journal) {
            log::warn!("unable to check rrd journal for an incomplete entry - {err}");
        }
        Ok(Self {
            config,
            journal,
            last_journal_flush: 0.0,
            journal_applied: false,
            apply_thread_result: None,
        })
    }

    pub fn sync_journal(&self) -> Result<(), Error> {
        nix::unistd::fdatasync(self.journal.as_raw_fd())?;
        Ok(())
    }

    pub fn append_journal_entry(
        &mut self,
        time: f64,
        value: f64,
        dst: DataSourceType,
        rel_path: &str,
    ) -> Result<(), Error> {
        let journal_entry = format!("{}:{}:{}:{}\n", time, value, dst as u8, rel_path);
        if let Err(err) = self.journal.write_all(journal_entry.as_bytes()) {
            if let Err(truncate_err) = truncate_incomplete_entry(&self.journal) {
                log::warn!("unable to drop incomplete rrd journal entry - {truncate_err}");
            }
            return Err(err.into());
        }
        Ok(())
    }

    pub fn open_journal_reader(&self) -> Result<BufReader<File>, Error> {
        // fixme : dup self.journal instead??
        let mut journal_path = self.config.basedir.clone();
        journal_path.push(RRD_JOURNAL_NAME);

        let flags = OFlag::O_CLOEXEC | OFlag::O_RDONLY;
        let journal =
            atomic_open_or_create_file(&journal_path, flags, &[], self.config.file_options, false)?;
        Ok(BufReader::new(journal))
    }

    fn open_journal_writer(config: &CacheConfig) -> Result<File, Error> {
        let mut journal_path = config.basedir.clone();
        journal_path.push(RRD_JOURNAL_NAME);

        let flags = OFlag::O_CLOEXEC | OFlag::O_RDWR | OFlag::O_APPEND;
        let journal =
            atomic_open_or_create_file(&journal_path, flags, &[], config.file_options, false)?;

        // Truncation does not reset the file offset. Enforce append mode even when the creation
        // helper returns a newly created temporary file without the requested status flags.
        let flags = OFlag::from_bits_truncate(fcntl(journal.as_raw_fd(), FcntlArg::F_GETFL)?);
        fcntl(
            journal.as_raw_fd(),
            FcntlArg::F_SETFL(flags | OFlag::O_APPEND),
        )?;
        Ok(journal)
    }

    pub fn rotate_journal(&mut self) -> Result<(), Error> {
        let mut journal_path = self.config.basedir.clone();
        journal_path.push(RRD_JOURNAL_NAME);

        let mut new_name = journal_path.clone();
        let now = proxmox_time::epoch_i64();
        new_name.set_extension(format!("journal-{now:08x}"));
        std::fs::rename(journal_path, &new_name)?;

        self.journal = Self::open_journal_writer(&self.config)?;

        // make sure the old journal data landed on the disk
        super::fsync_file_and_parent(&new_name)?;

        Ok(())
    }

    pub fn remove_old_journals(&self) -> Result<(), Error> {
        let journal_list = self.list_old_journals()?;

        for entry in journal_list {
            std::fs::remove_file(entry.path)?;
        }

        Ok(())
    }

    pub fn list_old_journals(&self) -> Result<Vec<JournalFileInfo>, Error> {
        let mut list = Vec::new();
        for entry in std::fs::read_dir(&self.config.basedir)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            match path.file_stem() {
                None => continue,
                Some(stem) if stem != OsStr::new("rrd") => continue,
                Some(_) => (),
            }

            if let Some(extension) = path.extension()
                && let Some(extension) = extension.to_str()
                && let Some(rest) = extension.strip_prefix("journal-")
                && let Ok(time) = u64::from_str_radix(rest, 16)
            {
                list.push(JournalFileInfo {
                    time,
                    name: format!("rrd.{extension}"),
                    path: path.to_owned(),
                });
            }
        }
        list.sort_unstable_by_key(|entry| entry.time);
        Ok(list)
    }
}

/// Truncate the journal after its last complete entry, so that the next append cannot continue an
/// entry left incomplete by a crash or failed write.
fn truncate_incomplete_entry(journal: &File) -> Result<(), Error> {
    let len = journal.metadata()?.len();
    let mut buf = [0u8; 4096];
    let mut end = len;

    while end > 0 {
        let start = end.saturating_sub(buf.len() as u64);
        let chunk = &mut buf[..(end - start) as usize];
        journal.read_exact_at(chunk, start)?;
        if let Some(pos) = chunk.iter().rposition(|&b| b == b'\n') {
            end = start + pos as u64 + 1;
            break;
        }
        end = start;
    }

    if end < len {
        let dropped = len - end;
        log::warn!("dropping incomplete rrd journal entry ({dropped} bytes)");
        journal.set_len(end)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::tests::TestDir;
    use nix::libc;
    use proxmox_sys::fs::CreateOptions;

    #[test]
    fn append_after_failed_write() {
        // File size limits and signal handlers are process-wide, so isolate fault injection from
        // other tests and their background threads.
        const CHILD_ENV: &str = "PROXMOX_RRD_TEST_SHORT_WRITE";
        if std::env::var_os(CHILD_ENV).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "cache::journal::tests::append_after_failed_write",
                ])
                .env(CHILD_ENV, "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }

        let dir = TestDir::new();
        let config = Arc::new(CacheConfig {
            basedir: dir.0.clone(),
            apply_interval: 1800.0,
            file_options: CreateOptions::new(),
            dir_options: CreateOptions::new(),
        });
        let mut state = JournalState::new(Arc::clone(&config)).unwrap();
        check_failed_append(&mut state);
        state.rotate_journal().unwrap();
        check_failed_append(&mut state);
        drop(state);
        let mut state = JournalState::new(config).unwrap();
        check_failed_append(&mut state);
    }

    fn check_failed_append(state: &mut JournalState) {
        state
            .append_journal_entry(1.0, 1.0, DataSourceType::Gauge, "host/cpu")
            .unwrap();
        let path = state.config.basedir.join(RRD_JOURNAL_NAME);
        let mut expected = std::fs::read(&path).unwrap();

        let result = unsafe {
            let mut original: libc::rlimit = std::mem::zeroed();
            assert_eq!(libc::getrlimit(libc::RLIMIT_FSIZE, &mut original), 0);
            let handler = libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            assert_ne!(handler, libc::SIG_ERR);
            let limit = libc::rlimit {
                rlim_cur: expected.len() as libc::rlim_t + 5,
                rlim_max: original.rlim_max,
            };
            assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
            let result = state.append_journal_entry(2.0, 2.0, DataSourceType::Gauge, "host/cpu");
            assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &original), 0);
            assert_ne!(libc::signal(libc::SIGXFSZ, handler), libc::SIG_ERR);
            result
        };
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), expected);

        state
            .append_journal_entry(3.0, 3.0, DataSourceType::Gauge, "host/cpu")
            .unwrap();
        expected.extend_from_slice(b"3:3:0:host/cpu\n");
        let actual = std::fs::read(&path).unwrap();
        assert_eq!(actual, expected);
        for line in std::str::from_utf8(&actual).unwrap().lines() {
            line.parse::<JournalEntry>().unwrap();
        }
    }

    fn check_truncate(name: &str, data: &[u8], expected: &[u8]) {
        let path = format!("./tests/testdata/journal-{name}.tmp");
        std::fs::write(&path, data).unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .append(true)
            .open(&path)
            .unwrap();
        truncate_incomplete_entry(&file).unwrap();
        let result = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(result, expected, "{name}");
    }

    #[test]
    fn truncate_incomplete_journal_entry() {
        let complete: &[u8] = b"1789798791.1:0.5:0:host/cpu\n1789798801.2:0.7:0:host/cpu\n";

        check_truncate("empty", b"", b"");
        check_truncate("complete", complete, complete);
        check_truncate("cut-time", &[complete, b"178979880"].concat(), complete);
        check_truncate("cut-path", &[complete, b"1:2:0:datas"].concat(), complete);
        check_truncate("no-newline", b"178979880", b"");
        check_truncate("zero-tail", &[complete, &[0u8; 5000]].concat(), complete);
    }
}
