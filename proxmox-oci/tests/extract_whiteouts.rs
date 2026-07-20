use std::fs::remove_dir_all;

use anyhow::Result;

use proxmox_oci::{Arch, parse_and_extract_image};
use proxmox_sys::fs::make_tmp_dir;

#[test]
fn test_whiteout_root_breakout() -> Result<()> {
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(
        &"tests/oci_image_data/oci_test_whiteout_root_breakout.tar".into(),
        &extract_dir,
        Some(&Arch::Amd64),
    )?;

    // Check that the whiteout did not remove the root directory
    assert!(extract_dir.exists());

    // Cleanup
    remove_dir_all(extract_dir)?;

    Ok(())
}

#[test]
fn test_whiteout_root_parent_breakout() -> Result<()> {
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(
        &"tests/oci_image_data/oci_test_whiteout_root_parent_breakout.tar".into(),
        &extract_dir,
        Some(&Arch::Amd64),
    )?;

    // Check that the whiteout did not remove the root directory
    assert!(extract_dir.exists());

    // Cleanup
    remove_dir_all(extract_dir)?;

    Ok(())
}

#[test]
fn test_whiteout_current_directory() -> Result<()> {
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(
        &"tests/oci_image_data/oci_test_whiteout_current_directory.tar".into(),
        &extract_dir,
        Some(&Arch::Amd64),
    )?;

    assert!(!extract_dir.join("etc").exists());

    // Cleanup
    remove_dir_all(extract_dir)?;

    Ok(())
}

#[test]
fn test_whiteout_symlink() -> Result<()> {
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(
        &"tests/oci_image_data/oci_test_whiteout_symlink.tar".into(),
        &extract_dir,
        Some(&Arch::Amd64),
    )?;

    assert!(extract_dir.join("etc/passwd").exists());
    assert!(!extract_dir.join("localetc").exists());

    // Cleanup
    remove_dir_all(extract_dir)?;

    Ok(())
}

#[test]
fn test_whiteout_dead_symlink_parent() -> Result<()> {
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(
        &"tests/oci_image_data/oci_test_whiteout_dead_symlink_parent.tar".into(),
        &extract_dir,
        Some(&Arch::Amd64),
    )?;

    assert!(extract_dir.join("etc/passwd").exists());

    // Cleanup
    remove_dir_all(extract_dir)?;

    Ok(())
}
