//! The command line of the deploy over adb: `kuula deploy push <cart> --to
//! adb[:serial] [--screenshot FILE]`, in a build with networking and in one
//! without, where it is all of `deploy` there is.

use std::path::Path;
#[cfg(not(feature = "net"))]
use std::path::PathBuf;

use kuula_core::{Snapshot, SnapshotLimits};
use kuula_host_common::carts::is_archive;
use kuula_mcp::session::clean_text;

use super::{check_name, deploy, Failure, Target};
use crate::{EXIT_FAULT, EXIT_OK, EXIT_USAGE};

/// The exit code when there is no adb or no device: the one a network
/// failure has, since this is the other way a cart does not get across.
const EXIT_DEVICE: u8 = 3;

/// `kuula deploy` in a build without networking: a push over adb is all
/// of it there is.
#[cfg(not(feature = "net"))]
#[derive(clap::Subcommand)]
pub enum OfflineDeploy {
    /// Push a cart to the Kuula app on an Android device over adb.
    Push {
        /// A cart directory, or a .zip/.cart file.
        cart: PathBuf,
        /// `adb`, or `adb:<serial>`.
        #[arg(long)]
        to: String,
        /// Write the device's screen to this PNG.
        #[arg(long, value_name = "FILE")]
        screenshot: Option<PathBuf>,
    },
    /// The rest of `deploy` is networking. Its words are taken, so that
    /// the answer says so and not that they were not understood.
    #[command(external_subcommand)]
    Other(#[allow(dead_code)] Vec<String>),
}

fn source_detail(e: &kuula_core::SourceError) -> String {
    if e.path.is_empty() {
        format!("{}: {}", e.code, e.message)
    } else {
        format!("{} {}: {}", e.code, e.path, e.message)
    }
}

/// The name and the package of a cart given on the command line: a
/// directory, packed as `kuula build` packs it, or a `.zip`/`.cart` file
/// as it is, once it has read as a cart. A path that is neither is
/// `usage`.
fn package_of(cart: &Path) -> Result<(String, Vec<u8>), Failure> {
    let limits = SnapshotLimits::default();
    let invalid = |e: kuula_core::SourceError| Failure::new("deploy_invalid", source_detail(&e));
    let (name, snap, packed) = if is_archive(cart) {
        let name = cart.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let snap = Snapshot::from_zip(cart, limits).map_err(invalid)?;
        let bytes = std::fs::read(cart).map_err(|e| {
            Failure::new(
                "deploy_invalid",
                format!("io_error: {}: {e}", cart.display()),
            )
        })?;
        (name.to_string(), snap, Some(bytes))
    } else if cart.is_dir() {
        // `.` and `..` have no name of their own.
        let full = std::fs::canonicalize(cart).unwrap_or_else(|_| cart.to_path_buf());
        let name = full.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let snap = Snapshot::from_dir(cart, limits).map_err(invalid)?;
        (name.to_string(), snap, None)
    } else {
        return Err(Failure::new(
            "usage",
            format!("{} is not a directory or a .zip/.cart file", cart.display()),
        ));
    };
    check_name(&name)?;
    if snap.get(kuula_core::console::MAIN_FILE).is_none() {
        return Err(Failure::new(
            "deploy_invalid",
            "not_found main.lua: a cart needs a main.lua",
        ));
    }
    let bytes = match packed {
        Some(bytes) => bytes,
        None => kuula_core::zipsource::pack(&snap).map_err(invalid)?,
    };
    Ok((name, bytes))
}

/// `kuula deploy push <cart> --to adb[:serial] [--screenshot FILE]`: the
/// report has the lines of a push to a receiver, then the app's log.
/// Exit codes: 0 pushed (and the screenshot written, when asked for), 1 a
/// cart that does not read, an app that is not there, a command that
/// failed or a screenshot that could not be had, 2 the cart's path or
/// name, 3 no adb or no device. Everything from the device is printed
/// with its control characters removed.
pub fn push_cli(cart: &Path, target: &Target, screenshot: Option<&Path>) -> u8 {
    let failed = |f: &Failure| {
        if f.code == "usage" {
            eprintln!("error: {}", f.detail);
            return EXIT_USAGE;
        }
        eprintln!("error: {} {}", f.code, clean_text(&f.detail));
        match f.code {
            "deploy_offer" => EXIT_USAGE,
            "adb_unavailable" | "adb_no_device" => EXIT_DEVICE,
            _ => EXIT_FAULT,
        }
    };
    let (name, package) = match package_of(cart) {
        Ok(p) => p,
        Err(f) => return failed(&f),
    };
    let report = match deploy(target, &name, &package, screenshot.is_some()) {
        Ok(report) => report,
        Err(f) => return failed(&f),
    };
    let say = |line: String| println!("{line}");
    say(format!(
        "push: {} {} bytes sha256 {}",
        report.name, report.bytes, report.digest
    ));
    say("transfer: ok".into());
    say("validation: ok".into());
    say("install: ok".into());
    if report.detail.is_empty() {
        say(format!("restart: {}", report.restart));
    } else {
        say(format!(
            "restart: {} {}",
            report.restart,
            clean_text(&report.detail)
        ));
    }
    for line in &report.log {
        say(format!("log: {}", clean_text(line)));
    }
    say("result: deploy_ok".into());
    // The cart is on the device either way; a screenshot that was asked
    // for and is not in the file is still a failure of what was asked.
    let Some(path) = screenshot else {
        return EXIT_OK;
    };
    let written = match (&report.screenshot, &report.no_screenshot) {
        (Some(png), _) => std::fs::write(path, png).map_err(|e| e.to_string()),
        (None, why) => Err(why.clone().unwrap_or_else(|| "none was taken".into())),
    };
    match written {
        Ok(()) => {
            say(format!("screenshot: {}", path.display()));
            EXIT_OK
        }
        Err(why) => {
            eprintln!(
                "error: no screenshot in {}: {}",
                path.display(),
                clean_text(&why)
            );
            EXIT_FAULT
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_cart_is_named_by_its_directory_and_packed() {
        let root = std::env::temp_dir().join(format!("kuula-adb-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cart = |name: &str, files: &[(&str, &str)]| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            for (file, text) in files {
                std::fs::write(dir.join(file), text).unwrap();
            }
            dir
        };
        let good = cart("my-cart", &[("main.lua", "function _update() end")]);
        let (name, package) = package_of(&good).unwrap();
        assert_eq!(name, "my-cart");
        // A zip, and the same one each time.
        assert!(package.starts_with(b"PK"));
        assert_eq!(package_of(&good).unwrap().1, package);

        // The packed file goes as it is, under its own name.
        let file = root.join("packed.cart");
        std::fs::write(&file, &package).unwrap();
        assert_eq!(package_of(&file).unwrap(), ("packed".to_string(), package));

        let code = |path: &Path| package_of(path).unwrap_err().code;
        assert_eq!(code(&cart("Bad Name", &[("main.lua", "")])), "deploy_offer");
        assert_eq!(
            code(&cart("no-main", &[("other.lua", "")])),
            "deploy_invalid"
        );
        assert_eq!(code(&root.join("not-there")), "usage");
        std::fs::write(root.join("broken.cart"), b"nope").unwrap();
        assert_eq!(code(&root.join("broken.cart")), "deploy_invalid");
        let _ = std::fs::remove_dir_all(&root);
    }
}
