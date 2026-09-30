use std::path::PathBuf;

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
