use std::fmt;
use std::path::Path;
use std::str::FromStr;
use std::{io::Write, path::PathBuf};

use anyhow::{Error, bail, format_err};
use regex::Regex;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use proxmox_apt::repositories;
use proxmox_apt_api_types::{
    APTRepositoryFile, APTRepositoryPackageType, APTUpdateInfo, DebianCodename,
};

/// Kernel series that the Proxmox products ship for Debian bookworm, i.e. Proxmox VE 8, Proxmox
/// Backup Server 3 and Proxmox Datacenter Manager 0.x.
pub const DEFAULT_PRE_UPGRADE_KERNELS: &[KernelSeries] = &[
    KernelSeries::new(6, 2),
    KernelSeries::new(6, 5),
    KernelSeries::new(6, 8),
    KernelSeries::new(6, 11),
    KernelSeries::new(6, 14),
];

/// Oldest kernel version that the Proxmox products ship for Debian trixie, i.e. Proxmox VE 9,
/// Proxmox Backup Server 4 and Proxmox Datacenter Manager 1.x.
///
/// The patch level is part of it because the bookworm builds of the 6.14 kernel only carry the
/// `bpo12` marker since 6.14.5, the older ones are indistinguishable from a trixie build otherwise.
pub const DEFAULT_MIN_POST_UPGRADE_KERNEL: KernelVersion = KernelVersion::new(6, 14, 5);

/// The `major.minor` part of a kernel version, e.g. `6.14`.
///
/// Proxmox ships one kernel meta-package per series, like `proxmox-kernel-6.14`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KernelSeries {
    pub major: u32,
    pub minor: u32,
}

impl KernelSeries {
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }
}

impl From<(u32, u32)> for KernelSeries {
    fn from((major, minor): (u32, u32)) -> Self {
        Self::new(major, minor)
    }
}

impl fmt::Display for KernelSeries {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl FromStr for KernelSeries {
    type Err = Error;

    /// Parse a bare `major.minor`, rejecting any trailing part.
    ///
    /// That keeps meta-packages like `proxmox-kernel-6.14` apart from anything else sharing their
    /// prefix, be it an image package or something like `proxmox-kernel-7.0-build-deps`.
    fn from_str(series: &str) -> Result<Self, Error> {
        let Some((major, minor)) = series.split_once('.') else {
            bail!("cannot parse kernel series '{series}' - expected 'major.minor'");
        };
        let parse = |part: &str, what: &str| -> Result<u32, Error> {
            part.parse::<u32>()
                .map_err(|err| format_err!("bad {what} version in '{series}' - {err}"))
        };

        Ok(Self::new(parse(major, "major")?, parse(minor, "minor")?))
    }
}

/// An upstream kernel version, e.g. `6.14.5`, ordered from oldest to newest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KernelVersion {
    pub series: KernelSeries,
    pub patch: u32,
}

impl KernelVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            series: KernelSeries::new(major, minor),
            patch,
        }
    }
}

impl From<KernelSeries> for KernelVersion {
    /// The oldest version of a series, as a series on its own says nothing about the patch level.
    fn from(series: KernelSeries) -> Self {
        Self { series, patch: 0 }
    }
}

impl fmt::Display for KernelVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.series, self.patch)
    }
}

impl FromStr for KernelVersion {
    type Err = Error;

    fn from_str(version: &str) -> Result<Self, Error> {
        // strip the epoch and everything after the upstream version, both only exist on packages
        let numbers = version.split_once(':').map_or(version, |(_, rest)| rest);
        let numbers = numbers
            .split_once('-')
            .map_or(numbers, |(numbers, _)| numbers);

        let mut numbers = numbers.split('.');
        let (Some(major), Some(minor)) = (numbers.next(), numbers.next()) else {
            bail!("cannot parse kernel version '{version}' - expected at least 'major.minor'");
        };
        // vendor and mainline kernels do not always have a patch level
        let patch = numbers.next().unwrap_or("0");

        // a component can carry a suffix, like Debian's `6.12.43+deb13-amd64` does
        let parse = |part: &str, what: &str| -> Result<u32, Error> {
            let digits = part
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap_or_default();

            digits
                .parse::<u32>()
                .map_err(|err| format_err!("bad {what} version in '{version}' - {err}"))
        };

        Ok(Self {
            series: KernelSeries::new(parse(major, "major")?, parse(minor, "minor")?),
            patch: parse(patch, "patch")?,
        })
    }
}

/// A kernel as a system reports it, i.e. a version plus the Debian release it was built for.
///
/// `uname -r` shows `6.14.11-9-pve` for a trixie build and `6.14.11-9-bpo12-pve` for the bookworm
/// backport of the same kernel, their package versions are `6.14.11-9` and `6.14.11-9~bpo12+1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelRelease {
    pub version: KernelVersion,
    /// The Debian major release this kernel got built for, if it is a backport at all.
    pub backport: Option<u32>,
}

impl FromStr for KernelRelease {
    type Err = Error;

    fn from_str(release: &str) -> Result<Self, Error> {
        Ok(Self {
            version: release.parse()?,
            backport: backport_marker(release),
        })
    }
}

/// Get the Debian major release a kernel got built for, if it carries a `bpo<N>` backport marker.
///
/// Proxmox marks the release in `uname -r` as `6.14.11-9-bpo12-pve` and in the matching package
/// version as `6.14.11-9~bpo12+1`. Debian's own backports have no release in the marker of their
/// release string, like `6.16.12+bpo-amd64`, so those simply do not match.
fn backport_marker(version: &str) -> Option<u32> {
    version
        .split(['.', '-', '~', '+', ':'])
        .find_map(|part| part.strip_prefix("bpo")?.parse().ok())
}

/// The Debian major release of a suite, as the `bpo<N>` marker of a backport refers to it.
fn debian_release_major(suite: &str) -> Option<u32> {
    Some(match DebianCodename::try_from(suite).ok()? {
        DebianCodename::Bullseye => 11,
        DebianCodename::Bookworm => 12,
        DebianCodename::Trixie => 13,
        DebianCodename::Forky => 14,
        DebianCodename::Duke => 15,
        // no product upgrading from an older release is still supported, and the newer ones
        // cannot be known yet, so leave judging a backport marker against those to the caller
        _ => return None,
    })
}

/// Get the kernel of an installed Proxmox kernel meta-package, e.g. `proxmox-kernel-6.14`.
///
/// Returns `None` for any other package, in particular for the versioned kernel image packages.
fn kernel_meta_package_release(pkg: &APTUpdateInfo) -> Option<KernelRelease> {
    let series: KernelSeries = pkg
        .package
        .strip_prefix("proxmox-kernel-")
        .or_else(|| pkg.package.strip_prefix("pve-kernel-"))?
        .parse()
        .ok()?;

    // the installed version, not the candidate one, tells us what a reboot would boot into
    let installed = pkg.old_version.as_deref()?;

    let version = match installed.parse::<KernelVersion>() {
        Ok(version) if version.series == series => version,
        // transitional meta-packages have a versioning of their own, so only trust the name
        _ => series.into(),
    };

    Some(KernelRelease {
        version,
        backport: backport_marker(installed),
    })
}

fn running_kernel_release() -> Result<String, Error> {
    let output = std::process::Command::new("uname")
        .arg("-r")
        .output()
        .map_err(|err| format_err!("failed to retrieve running kernel version - {err}"))?;

    if !output.status.success() {
        bail!(
            "failed to retrieve running kernel version - uname {}",
            output.status
        );
    }

    Ok(std::str::from_utf8(&output.stdout)?.trim().to_string())
}

/// Easily create and configure an upgrade checker for Proxmox products.
pub struct UpgradeCheckerBuilder {
    old_suite: String,
    new_suite: String,
    meta_package_name: String,
    minimum_major_version: u8,
    minimum_minor_version: u8,
    minimum_pkgrel: u8,
    apt_state_file: Option<PathBuf>,
    api_server_package: Option<String>,
    running_api_server_version: String,
    services_list: Vec<String>,
    pre_upgrade_kernels: Vec<KernelSeries>,
    min_post_upgrade_kernel: KernelVersion,
}

impl UpgradeCheckerBuilder {
    /// Create a new UpgradeCheckerBuilder
    ///
    /// * `old_suite`: The Debian suite before the upgrade.
    /// * `new_suite`: The Debian suite after the upgrade.
    /// * `meta_package_name`: The name of the product's meta package.
    /// * `minimum_major_version`: The minimum major version before the upgrade.
    /// * `minimum_minor_version`: The minimum minor version before the upgrade.
    /// * `minimum_pkgrel`: The minimum package release before the upgrade.
    /// * `running_api_server_version`: The currently running API server version.
    pub fn new(
        old_suite: &str,
        new_suite: &str,
        meta_package_name: &str,
        minimum_major_version: u8,
        minimum_minor_version: u8,
        minimum_pkgrel: u8,
        running_api_server_version: &str,
    ) -> UpgradeCheckerBuilder {
        UpgradeCheckerBuilder {
            old_suite: old_suite.into(),
            new_suite: new_suite.into(),
            meta_package_name: meta_package_name.into(),
            minimum_major_version,
            minimum_minor_version,
            minimum_pkgrel,
            apt_state_file: None,
            api_server_package: None,
            running_api_server_version: running_api_server_version.into(),
            services_list: Vec::new(),
            pre_upgrade_kernels: DEFAULT_PRE_UPGRADE_KERNELS.to_vec(),
            min_post_upgrade_kernel: DEFAULT_MIN_POST_UPGRADE_KERNEL,
        }
    }

    /// Set the location of the APT state file.
    pub fn apt_state_file_location<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.apt_state_file = Some(PathBuf::from(path.as_ref()));
        self
    }

    /// Set the API server package name.
    pub fn api_server_package(mut self, api_server_package: &str) -> Self {
        self.api_server_package = Some(api_server_package.into());
        self
    }

    /// Add a service to the list of services that will be checked.
    pub fn add_service_to_checks(mut self, service_name: &str) -> Self {
        self.services_list.push(service_name.into());
        self
    }

    /// Set the kernel series that are suitable to run before the upgrade.
    ///
    /// Those are matched exactly, as the releases before an upgrade got a fixed set of kernels.
    /// Defaults to [`DEFAULT_PRE_UPGRADE_KERNELS`].
    pub fn pre_upgrade_kernels<I>(mut self, series: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<KernelSeries>,
    {
        self.pre_upgrade_kernels = series.into_iter().map(Into::into).collect();
        self
    }

    /// Set the oldest kernel version that is suitable to run after the upgrade.
    ///
    /// Any newer one passes too, so that kernels released after this check was written are
    /// accepted, but a kernel backported to a Debian release older than the new suite never is.
    /// Spell out the patch level, it is what tells apart the builds of a series that got shipped
    /// for both the old and the new release.
    /// Defaults to [`DEFAULT_MIN_POST_UPGRADE_KERNEL`].
    pub fn min_post_upgrade_kernel(mut self, version: KernelVersion) -> Self {
        self.min_post_upgrade_kernel = version;
        self
    }

    /// Construct the UpgradeChecker, consumes the UpgradeCheckerBuilder
    pub fn build(mut self) -> UpgradeChecker {
        UpgradeChecker {
            output: ConsoleOutput::new(),
            upgraded: false,
            old_suite: self.old_suite,
            new_suite: self.new_suite,
            minimum_major_version: self.minimum_major_version,
            minimum_minor_version: self.minimum_minor_version,
            minimum_pkgrel: self.minimum_pkgrel,
            apt_state_file: self.apt_state_file.take().unwrap_or_else(|| {
                PathBuf::from(format!(
                    "/var/lib/{}/pkg-state.json",
                    &self.meta_package_name
                ))
            }),
            api_server_package: self
                .api_server_package
                .unwrap_or_else(|| self.meta_package_name.clone()),
            running_api_server_version: self.running_api_server_version,
            meta_package_name: self.meta_package_name,
            services_list: self.services_list,
            pre_upgrade_kernels: self.pre_upgrade_kernels,
            min_post_upgrade_kernel: self.min_post_upgrade_kernel,
        }
    }
}

/// Helpers to easily construct a set of upgrade checks.
pub struct UpgradeChecker {
    output: ConsoleOutput,
    upgraded: bool,
    old_suite: String,
    new_suite: String,
    meta_package_name: String,
    minimum_major_version: u8,
    minimum_minor_version: u8,
    minimum_pkgrel: u8,
    apt_state_file: PathBuf,
    api_server_package: String,
    running_api_server_version: String,
    services_list: Vec<String>,
    pre_upgrade_kernels: Vec<KernelSeries>,
    min_post_upgrade_kernel: KernelVersion,
}

impl UpgradeChecker {
    /// Run all checks.
    pub fn run(&mut self) -> Result<(), Error> {
        self.check_packages()?;
        self.check_misc()?;
        self.summary()
    }

    /// Run miscellaneous checks.
    pub fn check_misc(&mut self) -> Result<(), Error> {
        self.output.print_header("MISCELLANEOUS CHECKS")?;
        self.check_services()?;
        self.check_time_sync()?;
        self.check_apt_repos()?;
        self.check_bootloader()?;
        self.check_dkms_modules()?;
        Ok(())
    }

    /// Print a summary of all checks run so far.
    pub fn summary(&mut self) -> Result<(), Error> {
        self.output.print_summary()
    }

    /// Run all package related checks.
    pub fn check_packages(&mut self) -> Result<(), Error> {
        self.output.print_header(&format!(
            "CHECKING VERSION INFORMATION FOR {} PACKAGES",
            self.meta_package_name.to_uppercase()
        ))?;

        self.check_upgradable_packages()?;

        let pkg_versions = proxmox_apt::get_package_versions(
            &self.meta_package_name,
            &self.api_server_package,
            &self.running_api_server_version,
            &[],
        )?;

        self.check_meta_package_version(&pkg_versions)?;
        self.check_kernel_compat(&pkg_versions)?;
        Ok(())
    }

    fn check_upgradable_packages(&mut self) -> Result<(), Error> {
        self.output.log_info("Checking for package updates..")?;

        let result = proxmox_apt::list_available_apt_update(&self.apt_state_file);
        match result {
            Err(err) => {
                self.output.log_warn(format!("{err}"))?;
                self.output
                    .log_fail("unable to retrieve list of package updates!")?;
            }
            Ok(package_status) => {
                if package_status.is_empty() {
                    self.output.log_pass("all packages up-to-date")?;
                } else {
                    let pkgs = package_status
                        .iter()
                        .map(|pkg| pkg.package.clone())
                        .collect::<Vec<String>>()
                        .join(", ");
                    self.output.log_warn(format!(
                        "updates for the following packages are available:\n      {pkgs}",
                    ))?;
                }
            }
        }
        Ok(())
    }

    fn check_meta_package_version(&mut self, pkg_versions: &[APTUpdateInfo]) -> Result<(), Error> {
        self.output.log_info(format!(
            "Checking {} package version..",
            self.meta_package_name
        ))?;

        let meta_pkg = pkg_versions
            .iter()
            .find(|pkg| pkg.package.as_str() == self.meta_package_name);

        if let Some(old_version) = meta_pkg.and_then(|m| m.old_version.as_ref()) {
            let pkg_version = Regex::new(r"^(\d+)\.(\d+)[.-](\d+)")?;
            let captures = pkg_version.captures(old_version);
            if let Some(captures) = captures {
                let maj = Self::extract_version_from_captures(1, &captures)?;
                let min = Self::extract_version_from_captures(2, &captures)?;
                let pkgrel = Self::extract_version_from_captures(3, &captures)?;

                let min_version = format!(
                    "{}.{}.{}",
                    self.minimum_major_version, self.minimum_minor_version, self.minimum_pkgrel
                );

                if (maj > self.minimum_major_version && self.minimum_major_version != 0)
                    // Handle alpha and beta version upgrade checks:
                    || (self.minimum_major_version == 0 && min > self.minimum_minor_version)
                {
                    self.output
                        .log_pass(format!("Already upgraded to {maj}.{min}"))?;
                    self.upgraded = true;
                } else if maj >= self.minimum_major_version
                    && min >= self.minimum_minor_version
                    && pkgrel >= self.minimum_pkgrel
                {
                    self.output.log_pass(format!(
                        "'{}' has version >= {min_version}",
                        self.meta_package_name
                    ))?;
                } else {
                    self.output.log_fail(format!(
                        "'{}' package is too old, please upgrade to >= {min_version}",
                        self.meta_package_name
                    ))?;
                }
            } else {
                self.output.log_fail(format!(
                    "could not match the '{}' package version, \
                    is it installed?",
                    self.meta_package_name
                ))?;
            }
        } else {
            self.output
                .log_fail(format!("'{}' package not found!", self.meta_package_name))?;
        }
        Ok(())
    }

    /// Whether a kernel is one the product expects to run at the current stage of the upgrade.
    fn is_kernel_suitable(&self, kernel: KernelRelease) -> bool {
        if !self.upgraded {
            return self.pre_upgrade_kernels.contains(&kernel.version.series);
        }
        // a kernel built for an older Debian release than the new suite predates the upgrade, an
        // unknown suite keeps that conservative, as backports mostly go to the older release
        let predates_upgrade = kernel.backport.is_some_and(|backport| {
            debian_release_major(&self.new_suite).is_none_or(|new_release| backport < new_release)
        });

        !predates_upgrade && kernel.version >= self.min_post_upgrade_kernel
    }

    /// Get the newest installed kernel meta-package that would be suitable to run.
    fn find_suitable_installed_kernel<'a>(
        &self,
        pkg_versions: &'a [APTUpdateInfo],
    ) -> Option<&'a str> {
        pkg_versions
            .iter()
            .filter_map(|pkg| {
                let kernel = kernel_meta_package_release(pkg)?;
                self.is_kernel_suitable(kernel)
                    .then_some((kernel.version, pkg.package.as_str()))
            })
            .max_by_key(|(version, _)| *version)
            .map(|(_, package)| package)
    }

    fn check_kernel_compat(&mut self, pkg_versions: &[APTUpdateInfo]) -> Result<(), Error> {
        self.output.log_info("Check running kernel version..")?;

        let running_version = match running_kernel_release() {
            Ok(running_version) => running_version,
            Err(err) => {
                self.output.log_fail(err.to_string())?;
                return Ok(());
            }
        };

        let suitable = running_version
            .parse::<KernelRelease>()
            .is_ok_and(|kernel| self.is_kernel_suitable(kernel));

        if suitable {
            if self.upgraded {
                self.output.log_pass(format!(
                    "running new kernel '{running_version}' after upgrade."
                ))?;
            } else {
                self.output.log_pass(format!(
                    "running kernel '{running_version}' is considered suitable for upgrade."
                ))?;
            }
        } else if let Some(installed) = self.find_suitable_installed_kernel(pkg_versions) {
            self.output.log_warn(format!(
                "a suitable kernel '{installed}' is installed, but an unsuitable \
                '{running_version}' is booted, missing reboot?!",
            ))?;
        } else {
            self.output.log_warn(format!(
                "unexpected running and installed kernel '{running_version}'.",
            ))?;
        }
        Ok(())
    }

    fn extract_version_from_captures(
        index: usize,
        captures: &regex::Captures,
    ) -> Result<u8, Error> {
        if let Some(capture) = captures.get(index) {
            let val = capture.as_str().parse::<u8>()?;
            Ok(val)
        } else {
            Ok(0)
        }
    }

    fn check_bootloader(&mut self) -> Result<(), Error> {
        self.output
            .log_info("Checking bootloader configuration...")?;

        let sd_boot_installed =
            Path::new("/usr/share/doc/systemd-boot/changelog.Debian.gz").is_file();

        if !Path::new("/sys/firmware/efi").is_dir() {
            if sd_boot_installed {
                self.output.log_warn(
                    "systemd-boot package installed on legacy-boot system is not \
                    necessary, consider removing it",
                )?;
                return Ok(());
            }
            self.output
                .log_skip("System booted in legacy-mode - no need for additional packages.")?;
            return Ok(());
        }

        let mut boot_ok = true;
        if Path::new("/etc/kernel/proxmox-boot-uuids").is_file() {
            // Package version check needs to be run before
            if !self.upgraded {
                let output = std::process::Command::new("proxmox-boot-tool")
                    .arg("status")
                    .output()
                    .map_err(|err| {
                        format_err!("failed to retrieve proxmox-boot-tool status - {err}")
                    })?;
                let re = Regex::new(r"configured with:.* (uefi|systemd-boot) \(versions:")
                    .expect("failed to proxmox-boot-tool status");
                if re.is_match(std::str::from_utf8(&output.stdout)?) {
                    self.output
                        .log_skip("not yet upgraded, systemd-boot still needed for bootctl")?;
                    return Ok(());
                }
            }
        } else {
            if !Path::new("/usr/share/doc/grub-efi-amd64/changelog.Debian.gz").is_file() {
                self.output.log_warn(
                    "System booted in uefi mode but grub-efi-amd64 meta-package not installed, \
                     new grub versions will not be installed to /boot/efi!
                     Install grub-efi-amd64.",
                )?;
                boot_ok = false;
            }
            if Path::new("/boot/efi/EFI/BOOT/BOOTX64.efi").is_file() {
                let output = std::process::Command::new("debconf-show")
                    .arg("--db")
                    .arg("configdb")
                    .arg("grub-efi-amd64")
                    .arg("grub-pc")
                    .output()
                    .map_err(|err| format_err!("failed to retrieve debconf settings - {err}"))?;
                let re = Regex::new(r"grub2/force_efi_extra_removable: +true(?:\n|$)")
                    .expect("failed to compile dbconfig regex");
                if !re.is_match(std::str::from_utf8(&output.stdout)?) {
                    self.output.log_warn(
                        "Removable bootloader found at '/boot/efi/EFI/BOOT/BOOTX64.efi', but GRUB packages \
                        not set up to update it!\nRun the following command:\n\
                        echo 'grub-efi-amd64 grub2/force_efi_extra_removable boolean true' | debconf-set-selections -v -u\n\
                        Then reinstall GRUB with 'apt install --reinstall grub-efi-amd64'"
                    )?;
                    boot_ok = false;
                }
            }
        }
        if sd_boot_installed {
            self.output.log_fail(
                "systemd-boot meta-package installed. This will cause problems on upgrades of other \
                boot-related packages.\n\
                Remove the 'systemd-boot' package.\n\
                Please consult the upgrade guide for further information!"
            )?;
            boot_ok = false;
        }
        if boot_ok {
            self.output
                .log_pass("bootloader packages installed correctly")?;
        }
        Ok(())
    }

    fn check_apt_repos(&mut self) -> Result<(), Error> {
        self.output
            .log_info("Checking for package repository suite mismatches..")?;

        let mut strange_suite = false;
        let mut mismatches = Vec::new();
        let mut found_suite: Option<(String, String)> = None;

        let (repo_files, _repo_errors, _digest) = repositories::repositories()?;
        for repo_file in repo_files {
            self.check_repo_file(
                &mut found_suite,
                &mut mismatches,
                &mut strange_suite,
                repo_file,
            )?;
        }

        match (mismatches.is_empty(), strange_suite) {
            (true, false) => self.output.log_pass("found no suite mismatch")?,
            (true, true) => self
                .output
                .log_notice("found no suite mismatches, but found at least one strange suite")?,
            (false, _) => {
                let mut message = String::from(
                    "Found mixed old and new packages repository suites, fix before upgrading!\
                    \n      Mismatches:",
                );
                for (suite, location) in mismatches.iter() {
                    message.push_str(
                        format!("\n      found suite '{suite}' at '{location}'").as_str(),
                    );
                }
                message.push('\n');
                self.output.log_fail(message)?
            }
        }

        Ok(())
    }

    fn check_dkms_modules(&mut self) -> Result<(), Error> {
        let kver = std::process::Command::new("uname")
            .arg("-r")
            .output()
            .map_err(|err| format_err!("failed to retrieve running kernel version - {err}"))?;

        let output = std::process::Command::new("dkms")
            .arg("status")
            .arg("-k")
            .arg(std::str::from_utf8(&kver.stdout)?)
            .output();
        match output {
            Err(_err) => self.output.log_skip("could not get dkms status")?,
            Ok(ret) => {
                let num_dkms_modules = std::str::from_utf8(&ret.stdout)?.lines().count();
                if num_dkms_modules == 0 {
                    self.output.log_pass("no dkms modules found")?;
                } else {
                    self.output
                        .log_warn("dkms modules found, this might cause issues during upgrade.")?;
                }
            }
        }
        Ok(())
    }

    fn check_repo_file(
        &mut self,
        found_suite: &mut Option<(String, String)>,
        mismatches: &mut Vec<(String, String)>,
        strange_suite: &mut bool,
        repo_file: APTRepositoryFile,
    ) -> Result<(), Error> {
        for repo in repo_file.repositories {
            if !repo.enabled || repo.types == [APTRepositoryPackageType::DebSrc] {
                continue;
            }
            for suite in &repo.suites {
                let suite = match suite.find(&['-', '/'][..]) {
                    Some(n) => &suite[0..n],
                    None => suite,
                };

                if suite != self.old_suite && suite != self.new_suite {
                    let location = repo_file.path.clone().unwrap_or_default();
                    self.output.log_notice(format!(
                        "found unusual suite '{suite}', neither old '{}' nor new \
                            '{}'..\n        Affected file {location}\n        Please \
                            assure this is shipping compatible packages for the upgrade!",
                        self.old_suite, self.new_suite
                    ))?;
                    *strange_suite = true;
                    continue;
                }

                if let Some((current_suite, current_location)) = found_suite {
                    let location = repo_file.path.clone().unwrap_or_default();
                    if suite != current_suite {
                        if mismatches.is_empty() {
                            mismatches.push((current_suite.clone(), current_location.clone()));
                            mismatches.push((suite.to_string(), location));
                        } else {
                            mismatches.push((suite.to_string(), location));
                        }
                    }
                } else {
                    let location = repo_file.path.clone().unwrap_or_default();
                    *found_suite = Some((suite.to_string(), location));
                }
            }
        }
        Ok(())
    }

    fn get_systemd_unit_state(
        &self,
        unit: &str,
    ) -> Result<(SystemdUnitState, SystemdUnitState), Error> {
        let output = std::process::Command::new("systemctl")
            .arg("is-enabled")
            .arg(unit)
            .output()
            .map_err(|err| format_err!("failed to execute - {err}"))?;

        let enabled_state = match output.stdout.as_slice() {
            b"enabled\n" => SystemdUnitState::Enabled,
            b"disabled\n" => SystemdUnitState::Disabled,
            _ => SystemdUnitState::Unknown,
        };

        let output = std::process::Command::new("systemctl")
            .arg("is-active")
            .arg(unit)
            .output()
            .map_err(|err| format_err!("failed to execute - {err}"))?;

        let active_state = match output.stdout.as_slice() {
            b"active\n" => SystemdUnitState::Active,
            b"inactive\n" => SystemdUnitState::Inactive,
            b"failed\n" => SystemdUnitState::Failed,
            _ => SystemdUnitState::Unknown,
        };
        Ok((enabled_state, active_state))
    }

    fn check_services(&mut self) -> Result<(), Error> {
        self.output.log_info(format!(
            "Checking {} daemon services..",
            self.meta_package_name
        ))?;

        for service in self.services_list.as_slice() {
            match self.get_systemd_unit_state(service)? {
                (_, SystemdUnitState::Active) => {
                    self.output
                        .log_pass(format!("systemd unit '{service}' is in state 'active'"))?;
                }
                (_, SystemdUnitState::Inactive) => {
                    self.output.log_fail(format!(
                        "systemd unit '{service}' is in state 'inactive'\
                            \n    Please check the service for errors and start it.",
                    ))?;
                }
                (_, SystemdUnitState::Failed) => {
                    self.output.log_fail(format!(
                        "systemd unit '{service}' is in state 'failed'\
                            \n    Please check the service for errors and start it.",
                    ))?;
                }
                (_, _) => {
                    self.output.log_fail(format!(
                        "systemd unit '{service}' is not in state 'active'\
                            \n    Please check the service for errors and start it.",
                    ))?;
                }
            }
        }
        Ok(())
    }

    fn check_time_sync(&mut self) -> Result<(), Error> {
        self.output
            .log_info("Checking for supported & active NTP service..")?;
        if self.get_systemd_unit_state("systemd-timesyncd.service")?.1 == SystemdUnitState::Active {
            self.output.log_warn(
                "systemd-timesyncd is not the best choice for time-keeping on servers, due to only \
                applying updates on boot.\
                \n       While not necessary for the upgrade it's recommended to use one of:\
                \n        * chrony (Default in new Proxmox product installations)\
                \n        * ntpsec\
                \n        * openntpd",
            )?;
        } else if self.get_systemd_unit_state("ntp.service")?.1 == SystemdUnitState::Active {
            self.output.log_info(
                "Debian deprecated and removed the ntp package for Bookworm, but the system \
                    will automatically migrate to the 'ntpsec' replacement package on upgrade.",
            )?;
        } else if self.get_systemd_unit_state("chrony.service")?.1 == SystemdUnitState::Active
            || self.get_systemd_unit_state("openntpd.service")?.1 == SystemdUnitState::Active
            || self.get_systemd_unit_state("ntpsec.service")?.1 == SystemdUnitState::Active
        {
            self.output
                .log_pass("Detected active time synchronisation unit")?;
        } else {
            self.output.log_warn(
                "No (active) time synchronisation daemon (NTP) detected, but synchronized systems \
                are important!",
            )?;
        }
        Ok(())
    }
}

#[derive(PartialEq)]
enum SystemdUnitState {
    Active,
    Enabled,
    Disabled,
    Failed,
    Inactive,
    Unknown,
}

#[derive(Default)]
struct Counters {
    pass: u64,
    skip: u64,
    notice: u64,
    warn: u64,
    fail: u64,
}

enum LogLevel {
    Pass,
    Info,
    Skip,
    Notice,
    Warn,
    Fail,
}

struct ConsoleOutput {
    stream: StandardStream,
    first_header: bool,
    counters: Counters,
}

impl ConsoleOutput {
    fn new() -> Self {
        Self {
            stream: StandardStream::stdout(ColorChoice::Always),
            first_header: true,
            counters: Counters::default(),
        }
    }

    fn print_header(&mut self, message: &str) -> Result<(), Error> {
        if !self.first_header {
            writeln!(&mut self.stream)?;
        }
        self.first_header = false;
        writeln!(&mut self.stream, "= {message} =\n")?;
        Ok(())
    }

    fn set_color(&mut self, color: Color, bold: bool) -> Result<(), Error> {
        self.stream
            .set_color(ColorSpec::new().set_fg(Some(color)).set_bold(bold))?;
        Ok(())
    }

    fn reset(&mut self) -> Result<(), std::io::Error> {
        self.stream.reset()
    }

    fn log_line(&mut self, level: LogLevel, message: &str) -> Result<(), Error> {
        match level {
            LogLevel::Pass => {
                self.counters.pass += 1;
                self.set_color(Color::Green, false)?;
                writeln!(&mut self.stream, "PASS: {message}")?;
            }
            LogLevel::Info => {
                writeln!(&mut self.stream, "INFO: {message}")?;
            }
            LogLevel::Skip => {
                self.counters.skip += 1;
                writeln!(&mut self.stream, "SKIP: {message}")?;
            }
            LogLevel::Notice => {
                self.counters.notice += 1;
                self.set_color(Color::White, true)?;
                writeln!(&mut self.stream, "NOTICE: {message}")?;
            }
            LogLevel::Warn => {
                self.counters.warn += 1;
                self.set_color(Color::Yellow, false)?;
                writeln!(&mut self.stream, "WARN: {message}")?;
            }
            LogLevel::Fail => {
                self.counters.fail += 1;
                self.set_color(Color::Red, true)?;
                writeln!(&mut self.stream, "FAIL: {message}")?;
            }
        }
        self.reset()?;
        Ok(())
    }

    fn log_pass<T: AsRef<str>>(&mut self, message: T) -> Result<(), Error> {
        self.log_line(LogLevel::Pass, message.as_ref())
    }

    fn log_info<T: AsRef<str>>(&mut self, message: T) -> Result<(), Error> {
        self.log_line(LogLevel::Info, message.as_ref())
    }

    fn log_skip<T: AsRef<str>>(&mut self, message: T) -> Result<(), Error> {
        self.log_line(LogLevel::Skip, message.as_ref())
    }

    fn log_notice<T: AsRef<str>>(&mut self, message: T) -> Result<(), Error> {
        self.log_line(LogLevel::Notice, message.as_ref())
    }

    fn log_warn<T: AsRef<str>>(&mut self, message: T) -> Result<(), Error> {
        self.log_line(LogLevel::Warn, message.as_ref())
    }

    fn log_fail<T: AsRef<str>>(&mut self, message: T) -> Result<(), Error> {
        self.log_line(LogLevel::Fail, message.as_ref())
    }

    fn print_summary(&mut self) -> Result<(), Error> {
        self.print_header("SUMMARY")?;

        let total = self.counters.fail
            + self.counters.pass
            + self.counters.notice
            + self.counters.skip
            + self.counters.warn;

        writeln!(&mut self.stream, "TOTAL:     {total}")?;
        self.set_color(Color::Green, false)?;
        writeln!(&mut self.stream, "PASSED:    {}", self.counters.pass)?;
        self.reset()?;
        writeln!(&mut self.stream, "SKIPPED:   {}", self.counters.skip)?;
        writeln!(&mut self.stream, "NOTICE:    {}", self.counters.notice)?;
        if self.counters.warn > 0 {
            self.set_color(Color::Yellow, false)?;
            writeln!(&mut self.stream, "WARNINGS:  {}", self.counters.warn)?;
        }
        if self.counters.fail > 0 {
            self.set_color(Color::Red, true)?;
            writeln!(&mut self.stream, "FAILURES:  {}", self.counters.fail)?;
        }
        if self.counters.warn > 0 || self.counters.fail > 0 {
            let (color, bold) = if self.counters.fail > 0 {
                (Color::Red, true)
            } else {
                (Color::Yellow, false)
            };

            self.set_color(color, bold)?;
            writeln!(
                &mut self.stream,
                "\nATTENTION: Please check the output for detailed information!",
            )?;
            if self.counters.fail > 0 {
                writeln!(
                    &mut self.stream,
                    "Try to solve the problems one at a time and rerun this checklist tool again.",
                )?;
            }
        }
        self.reset()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_checker(upgraded: bool) -> UpgradeChecker {
        let mut checker = UpgradeCheckerBuilder::new(
            "bookworm",
            "trixie",
            "proxmox-backup",
            3,
            4,
            0,
            "running version: 3.4",
        )
        .build();

        checker.upgraded = upgraded;
        checker
    }

    fn installed_kernel_package(package: &str, version: &str) -> APTUpdateInfo {
        APTUpdateInfo {
            package: package.to_string(),
            title: "kernel".to_string(),
            arch: "amd64".to_string(),
            description: "kernel".to_string(),
            version: version.to_string(),
            old_version: Some(version.to_string()),
            origin: "Proxmox".to_string(),
            priority: "optional".to_string(),
            section: "admin".to_string(),
            extra_info: None,
        }
    }

    fn test_is_kernel_version_compatible(
        expected_versions: &[&str],
        unexpected_versions: &[&str],
        upgraded: bool,
    ) {
        let checker = make_checker(upgraded);

        for version in expected_versions {
            let kernel = version
                .parse()
                .unwrap_or_else(|err| panic!("failed to parse kernel version '{version}' - {err}"));
            assert!(
                checker.is_kernel_suitable(kernel),
                "compatible kernel version '{version}' did not pass as expected!"
            );
        }
        for version in unexpected_versions {
            let suitable = version
                .parse()
                .is_ok_and(|kernel| checker.is_kernel_suitable(kernel));
            assert!(
                !suitable,
                "incompatible kernel version '{version}' passed as expected!"
            );
        }
    }

    #[test]
    fn test_before_upgrade_kernel_version_compatibility() {
        let expected_versions = &[
            "6.2.16-20-pve",
            "6.5.13-6-pve",
            "6.8.12-13-pve",
            "6.11.11-2-pve",
            // the opt-in 6.14 kernel for bookworm, before and after it got the backport marker
            "6.14.4-1-pve",
            "6.14.11-9-bpo12-pve",
        ];
        let unexpected_versions = &[
            "6.1.10-1-pve",
            "5.19.17-2-pve",
            "6.1.0-18-amd64",
            "6.17.2-1-pve",
            "not-a-kernel-version",
        ];

        test_is_kernel_version_compatible(expected_versions, unexpected_versions, false);
    }

    #[test]
    fn test_after_upgrade_kernel_version_compatibility() {
        let expected_versions = &[
            "6.14.5-1-pve",
            "6.14.11-9-pve",
            "6.17.2-1-pve",
            "6.20.1-1-pve",
            "7.0.14-6-pve",
            // Debian's own backports carry no release in the marker, so they are not from bookworm
            "6.16.12+bpo-amd64",
        ];
        let unexpected_versions = &[
            // Debian's own kernel is older than what the products ship, as it was before too
            "6.12.43+deb13-amd64",
            "6.8.12-13-pve",
            // the bookworm builds of 6.14 predating the backport marker
            "6.14.0-2-pve",
            "6.14.4-1-pve",
            // and those carrying it, which can be newer than the trixie kernel one rebooted into
            "6.14.11-9-bpo12-pve",
            "6.20.1-1-bpo12-pve",
            "not-a-kernel-version",
        ];

        test_is_kernel_version_compatible(expected_versions, unexpected_versions, true);
    }

    #[test]
    fn test_kernel_backported_to_the_new_suite_is_suitable() {
        // a kernel backported to the release upgraded *to* is a newer one, not a leftover
        let checker = make_checker(true);
        let kernel: KernelRelease = "6.20.1-1-bpo13-pve"
            .parse()
            .expect("failed to parse kernel");
        assert!(checker.is_kernel_suitable(kernel));

        // while for an upgrade to a later release the very same kernel is a leftover
        let mut checker = make_checker(true);
        checker.new_suite = "duke".to_string();
        assert!(!checker.is_kernel_suitable(kernel));

        // sid is no release, so without a Debian version to compare to any backport counts as one
        checker.new_suite = "sid".to_string();
        assert!(!checker.is_kernel_suitable(kernel));
    }

    fn parse_kernel(release: &str) -> KernelRelease {
        release
            .parse()
            .unwrap_or_else(|err| panic!("failed to parse kernel release '{release}' - {err}"))
    }

    #[test]
    fn test_parse_kernel_release() {
        let kernel = parse_kernel("6.14.11-9-pve");
        assert_eq!(kernel.version, KernelVersion::new(6, 14, 11));
        assert_eq!(kernel.backport, None);

        assert_eq!(parse_kernel("6.14.11-9-bpo12-pve").backport, Some(12));

        // package versions use different separators around the backport marker and have an epoch
        let kernel = parse_kernel("1:6.14.11-9~bpo12+1");
        assert_eq!(kernel.version, KernelVersion::new(6, 14, 11));
        assert_eq!(kernel.backport, Some(12));

        // Debian appends the release to the upstream version, and its backports have no release
        let kernel = parse_kernel("6.12.43+deb13-amd64");
        assert_eq!(kernel.version, KernelVersion::new(6, 12, 43));
        assert_eq!(kernel.backport, None);
        let kernel = parse_kernel("6.16.12+bpo-amd64");
        assert_eq!(kernel.version, KernelVersion::new(6, 16, 12));
        assert_eq!(kernel.backport, None);

        // a missing patch level is common for mainline and vendor kernels
        assert_eq!(parse_kernel("7.0-rc1").version, KernelVersion::new(7, 0, 0));

        for release in ["", "6", "6.x.1-1-pve", "+6.14.1-1-pve", "linux"] {
            assert!(
                release.parse::<KernelRelease>().is_err(),
                "bogus kernel release '{release}' parsed as expected!"
            );
        }
    }

    #[test]
    fn test_kernel_versions_order_by_age() {
        let mut versions = [
            KernelVersion::new(6, 14, 11),
            KernelVersion::new(7, 0, 14),
            KernelVersion::new(6, 2, 16),
            KernelVersion::new(6, 14, 5),
        ];
        versions.sort();

        assert_eq!(
            versions,
            [
                KernelVersion::new(6, 2, 16),
                KernelVersion::new(6, 14, 5),
                KernelVersion::new(6, 14, 11),
                KernelVersion::new(7, 0, 14),
            ]
        );
        assert_eq!(
            KernelVersion::from(KernelSeries::new(6, 14)),
            KernelVersion::new(6, 14, 0)
        );
    }

    #[test]
    fn test_find_suitable_installed_kernel() {
        let packages = &[
            installed_kernel_package("proxmox-kernel-helper", "9.2.0"),
            installed_kernel_package("proxmox-kernel-libc-dev", "7.0.14-2"),
            installed_kernel_package("proxmox-kernel-7.0-build-deps", "7.0.6-1"),
            installed_kernel_package("proxmox-kernel-6.8", "6.8.12-16"),
            installed_kernel_package("proxmox-kernel-6.14", "6.14.11-9"),
            installed_kernel_package("proxmox-kernel-6.14.11-9-pve-signed", "6.14.11-9"),
            installed_kernel_package("proxmox-kernel-6.17", "6.17.2-1"),
        ];

        let checker = make_checker(true);
        assert_eq!(
            checker.find_suitable_installed_kernel(packages),
            Some("proxmox-kernel-6.17")
        );

        let checker = make_checker(false);
        assert_eq!(
            checker.find_suitable_installed_kernel(packages),
            Some("proxmox-kernel-6.14")
        );

        // a backported meta-package is not suitable after the upgrade, but before it is
        let backport = &[installed_kernel_package(
            "proxmox-kernel-6.14",
            "6.14.11-9~bpo12+1",
        )];
        assert_eq!(
            make_checker(true).find_suitable_installed_kernel(backport),
            None
        );
        assert_eq!(
            make_checker(false).find_suitable_installed_kernel(backport),
            Some("proxmox-kernel-6.14")
        );

        // as is one that is too old, here the bookworm 6.14 from before the backport marker
        let old = &[installed_kernel_package("proxmox-kernel-6.14", "6.14.4-1")];
        assert_eq!(make_checker(true).find_suitable_installed_kernel(old), None);
        assert_eq!(
            make_checker(false).find_suitable_installed_kernel(old),
            Some("proxmox-kernel-6.14")
        );
    }
}
