use super::*;
use crate::rrd::Archive;

pub(super) struct TestDir(pub PathBuf);

impl TestDir {
    pub fn new() -> Self {
        Self(proxmox_sys::fs::make_tmp_dir("./tests/testdata", None).unwrap())
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn new_cache(path: &Path) -> Cache {
    Cache::new(
        path,
        None,
        None,
        f64::INFINITY,
        |path, _| Database::load(path, false).ok(),
        |dst| Database::new(dst, vec![Archive::new(AggregationFn::Last, 10, 20)]),
    )
    .unwrap()
}

#[test]
fn reject_invalid_metric_paths() {
    let dir = TestDir::new();
    let cache = new_cache(&dir.0);
    for path in [
        "",
        "/host/cpu",
        "../host/cpu",
        "host/..",
        "host/../cpu",
        "./host/cpu",
        "host/./cpu",
        "host//cpu",
        "host/",
        "host/cpu\0",
        "host/cpu\r",
        "host/cpu\t",
        "host/cpu\u{85}",
        "pve/remote/node/cpu\n2000000000:1:0:pve/other/node/cpu",
        "rrd.journal",
        "rrd.journal-1234",
        "rrd.journal/metric",
    ] {
        assert!(
            cache
                .update_value(path, 100.0, 1.0, DataSourceType::Gauge)
                .is_err(),
            "{path:?}"
        );
        assert!(
            cache
                .update_value_ignore_old(path, 100.0, 1.0, DataSourceType::Gauge)
                .is_err(),
            "{path:?}"
        );
        assert!(
            cache
                .extract_cached_data(path, "value", AggregationFn::Last, 10, None, None)
                .is_err(),
            "{path:?}"
        );
        assert!(
            cache
                .state
                .write()
                .unwrap()
                .append_journal_entry(100.0, 1.0, DataSourceType::Gauge, path)
                .is_err(),
            "{path:?}"
        );
        assert!(
            format!("100:1:0:{path}\n").parse::<JournalEntry>().is_err(),
            "{path:?}"
        );
    }
    let state = cache.state.read().unwrap();
    assert!(state.apply_thread_result.is_none());
    assert_eq!(state.open_journal_reader().unwrap().lines().count(), 0);
}

#[test]
fn replay_skips_invalid_paths() {
    let dir = TestDir::new();
    let outside = TestDir::new();
    let target = outside.0.canonicalize().unwrap().join("value");
    let journal = format!(
        "100:1:0:{}\n100:1:0:../value\n100:1:0:rrd.journal\n100:1:0:host/cpu\r\n100:2:0:host/cpu\n100:3:0:host/space \n",
        target.display(),
    );
    std::fs::write(dir.0.join("rrd.journal"), journal).unwrap();
    let cache = new_cache(&dir.0);
    apply_and_commit_journal_thread(
        Arc::clone(&cache.config),
        Arc::clone(&cache.state),
        Arc::clone(&cache.rrd_map),
        false,
    )
    .unwrap();
    assert!(!target.exists());
    assert_eq!(
        Database::load(&dir.0.join("host/cpu"), false)
            .unwrap()
            .source
            .last_value,
        2.0
    );
    assert_eq!(
        Database::load(&dir.0.join("host/space "), false)
            .unwrap()
            .source
            .last_value,
        3.0
    );
    assert!(!dir.0.join("host/space").exists());
    assert!(std::fs::read(dir.0.join("rrd.journal")).unwrap().is_empty());
}

#[test]
fn valid_metric_paths_round_trip() {
    let dir = TestDir::new();
    let cache = new_cache(&dir.0);
    let mut state = cache.state.write().unwrap();
    for path in [
        "host/cpu",
        "pve/remote/qemu/100/cpu",
        "host/a..b",
        "host/a:b",
        "host/space ",
    ] {
        state
            .append_journal_entry(100.0, 1.0, DataSourceType::Gauge, path)
            .unwrap();
        let entry: JournalEntry = format!("100:1:0:{path}\n").parse().unwrap();
        assert_eq!(entry.rel_path, path);
    }
}

#[test]
fn journal_round_trip_and_replay() {
    for dst in [
        DataSourceType::Gauge,
        DataSourceType::Derive,
        DataSourceType::Counter,
    ] {
        let dir = TestDir::new();
        let cache = new_cache(&dir.0);
        {
            let mut state = cache.state.write().unwrap();
            state
                .append_journal_entry(100.0, 100.0, dst, "host/value")
                .unwrap();
            state
                .append_journal_entry(110.0, 120.0, dst, "host/value")
                .unwrap();
            let lines = state
                .open_journal_reader()
                .unwrap()
                .lines()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(lines.len(), 2);
            for (line, (time, value)) in lines.iter().zip([(100.0, 100.0), (110.0, 120.0)]) {
                let entry: JournalEntry = line.parse().unwrap();
                assert_eq!(entry.dst, dst);
                assert_eq!(entry.time, time);
                assert_eq!(entry.value, value);
                assert_eq!(entry.rel_path, "host/value");
            }
        }
        drop(cache);

        let cache = new_cache(&dir.0);
        apply_and_commit_journal_thread(
            Arc::clone(&cache.config),
            Arc::clone(&cache.state),
            Arc::clone(&cache.rrd_map),
            false,
        )
        .unwrap();

        let database = Database::load(&dir.0.join("host/value"), false).unwrap();
        assert_eq!(database.source.dst, dst);
        assert_eq!(database.last_update(), 110.0);
        let entry = database
            .extract_data(AggregationFn::Last, 10, Some(110), Some(110))
            .unwrap();
        let expected = if dst == DataSourceType::Gauge {
            120.0
        } else {
            2.0
        };
        assert_eq!(entry.data, [Some(expected)]);
        assert!(
            cache
                .state
                .read()
                .unwrap()
                .list_old_journals()
                .unwrap()
                .is_empty()
        );
    }
}
