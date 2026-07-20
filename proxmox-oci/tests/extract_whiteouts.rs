use std::fs::remove_dir_all;

use anyhow::Result;

use proxmox_oci::{Arch, parse_and_extract_image};
use proxmox_sys::fs::make_tmp_dir;

mod helper;
use helper::{LayerEntry, OciTarImageBuilder};

#[test]
fn test_whiteout_root_breakout() -> Result<()> {
    let image_dir = make_tmp_dir("/tmp/", None)?;
    let image_path = image_dir.join("oci_image.tar");
    OciTarImageBuilder::default()
        .layer(vec![LayerEntry::file(".wh..", b"")])
        .build(&image_path)?;
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(&image_path, &extract_dir, Some(&Arch::Amd64))?;

    // Check that the whiteout did not remove the root directory
    assert!(extract_dir.exists());

    // Cleanup
    remove_dir_all(extract_dir)?;
    remove_dir_all(image_dir)?;

    Ok(())
}

#[test]
fn test_whiteout_root_parent_breakout() -> Result<()> {
    let image_dir = make_tmp_dir("/tmp/", None)?;
    let image_path = image_dir.join("oci_image.tar");
    OciTarImageBuilder::default()
        .layer(vec![LayerEntry::file(".wh...", b"")])
        .build(&image_path)?;
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(&image_path, &extract_dir, Some(&Arch::Amd64))?;

    // Check that the whiteout did not remove the root directory
    assert!(extract_dir.exists());

    // Cleanup
    remove_dir_all(extract_dir)?;
    remove_dir_all(image_dir)?;

    Ok(())
}

#[test]
fn test_whiteout_current_directory() -> Result<()> {
    let image_dir = make_tmp_dir("/tmp/", None)?;
    let image_path = image_dir.join("oci_image.tar");
    OciTarImageBuilder::default()
        .layer(vec![LayerEntry::file("etc/passwd", b"passwd")])
        .layer(vec![LayerEntry::file("etc/.wh..", b"")])
        .build(&image_path)?;
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(&image_path, &extract_dir, Some(&Arch::Amd64))?;

    assert!(!extract_dir.join("etc").exists());

    // Cleanup
    remove_dir_all(extract_dir)?;
    remove_dir_all(image_dir)?;

    Ok(())
}

#[test]
fn test_whiteout_symlink() -> Result<()> {
    let image_dir = make_tmp_dir("/tmp/", None)?;
    let image_path = image_dir.join("oci_image.tar");
    OciTarImageBuilder::default()
        .layer(vec![
            LayerEntry::file("etc/passwd", b"passwd"),
            LayerEntry::symlink("localetc", "etc"),
        ])
        .layer(vec![LayerEntry::file(".wh.localetc", b"")])
        .build(&image_path)?;
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(&image_path, &extract_dir, Some(&Arch::Amd64))?;

    assert!(extract_dir.join("etc/passwd").exists());
    assert!(!extract_dir.join("localetc").exists());

    // Cleanup
    remove_dir_all(extract_dir)?;
    remove_dir_all(image_dir)?;

    Ok(())
}

#[test]
fn test_whiteout_dead_symlink_parent() -> Result<()> {
    let image_dir = make_tmp_dir("/tmp/", None)?;
    let image_path = image_dir.join("oci_image.tar");
    OciTarImageBuilder::default()
        .layer(vec![
            LayerEntry::file("etc/passwd", b"passwd"),
            LayerEntry::symlink("localetc", "dne"),
        ])
        .layer(vec![LayerEntry::file("localetc/.wh.etc", b"")])
        .build(&image_path)?;
    let extract_dir = make_tmp_dir("/tmp/", None)?;

    parse_and_extract_image(&image_path, &extract_dir, Some(&Arch::Amd64))?;

    assert!(extract_dir.join("etc/passwd").exists());

    // Cleanup
    remove_dir_all(extract_dir)?;
    remove_dir_all(image_dir)?;

    Ok(())
}
