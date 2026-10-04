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
