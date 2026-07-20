use std::fs::{read_to_string, remove_dir_all};

use anyhow::Result;

use proxmox_oci::{Arch, parse_and_extract_image};
use proxmox_sys::fs::make_tmp_dir;

#[test]
fn test_replace_file() -> Result<()> {
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(
        &"tests/oci_image_data/oci_test_replace_file.tar".into(),
        &extract_dir,
        Some(&Arch::Amd64),
    )?;

    let replaced_path = extract_dir.join("etc/a");
    assert!(replaced_path.is_file());
    assert_eq!(read_to_string(replaced_path)?, "2");

    // Cleanup
    remove_dir_all(extract_dir)?;

    Ok(())
}

#[test]
fn test_replace_file_with_dir() -> Result<()> {
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(
        &"tests/oci_image_data/oci_test_replace_file_with_dir.tar".into(),
        &extract_dir,
        Some(&Arch::Amd64),
    )?;

    assert!(extract_dir.join("etc/a").is_dir());

    // Cleanup
    remove_dir_all(extract_dir)?;

    Ok(())
}

#[test]
fn test_replace_dir_with_file() -> Result<()> {
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(
        &"tests/oci_image_data/oci_test_replace_dir_with_file.tar".into(),
        &extract_dir,
        Some(&Arch::Amd64),
    )?;

    assert!(extract_dir.join("etc/a").is_file());

    // Cleanup
    remove_dir_all(extract_dir)?;

    Ok(())
}
