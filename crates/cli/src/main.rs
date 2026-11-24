//! `bootstick`: the core of the app on the command line, for testing on real
//! machines before the window exists.
//!
//!   bootstick manifest                 show the ISO of the proof of concept
//!   bootstick download                 download and check the ISO
//!   bootstick disks                    list the disks and which may be written (Windows)
//!   bootstick flash --disk 2           download, write, check, boot entry (Windows, admin)
//!   bootstick boot-entry --image F     show the boot entry the app would create for an image

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use bootstick_core::image::Image;
use bootstick_core::manifest::Manifest;
use bootstick_core::progress::{Observer, Progress, Stage};
use bootstick_core::{boot_entry, download, partition, release};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Write the Strix ISO to a USB stick")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the manifest of the ISO
    Manifest,
    /// Download the ISO and check its SHA256
    Download {
        /// Folder for the download (default: a folder in the temp directory)
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// List the disks and whether they may be written
    Disks,
    /// Download, write to the stick, read back, set the boot entry
    Flash {
        /// Number of the disk, from `bootstick disks`
        #[arg(long)]
        disk: u32,
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Use this ISO file instead of downloading (its SHA256 must be given)
        #[arg(long, requires = "sha256")]
        image: Option<PathBuf>,
        #[arg(long)]
        sha256: Option<String>,
        /// Skip reading the stick back
        #[arg(long)]
        no_verify: bool,
        /// Leave the firmware boot entries alone
        #[arg(long)]
        no_boot_entry: bool,
        /// Check everything, write nothing
        #[arg(long)]
        dry_run: bool,
        /// Do not ask before deleting the stick
        #[arg(long)]
        yes: bool,
    },
    /// Show the EFI partition and the boot entry for an image file
    BootEntry {
        #[arg(long)]
        image: PathBuf,
        /// Description of the entry
        #[arg(long, default_value = "Installer")]
        name: String,
    },
}

/// Prints progress on one line
struct Console {
    last: Option<(Stage, u64)>,
}

impl Observer for Console {
    fn progress(&mut self, p: &Progress) {
        let percent = (p.done * 100).checked_div(p.total).unwrap_or(100);
        if self.last == Some((p.stage, percent)) {
            return;
        }
        self.last = Some((p.stage, percent));
        let what = match p.stage {
            Stage::Download => format!("Downloading {}", p.file),
            Stage::Write => "Writing".to_owned(),
            Stage::Verify => "Checking".to_owned(),
        };
        eprint!("\r{what}: {percent:3}%   ");
        if percent == 100 {
            eprintln!();
        }
        let _ = io::stderr().flush();
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("\nError: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> bootstick_core::Result<()> {
    match cli.command {
        Command::Manifest => {
            let m = Manifest::fetch(&release::agent())?;
            println!("{} {} ({})", m.name, m.version, m.released);
            for d in m.downloads() {
                println!(
                    "  {} {} bytes\n    {}\n    sha256 {}",
                    d.file, d.size, d.url, d.sha256
                );
            }
            println!(
                "  needs: UEFI {}, Secure Boot off {}, {} GiB RAM, {} GiB disk",
                m.requirements.uefi,
                m.requirements.secure_boot_off,
                m.requirements.ram_gib,
                m.requirements.disk_gib
            );
            Ok(())
        }
        Command::Download { dir } => {
            let agent = release::agent();
            let m = Manifest::fetch(&agent)?;
            let dir = dir.unwrap_or_else(|| download::cache_dir(&m));
            let paths = download::download_release(&agent, &m, &dir, &mut Console { last: None })?;
            for p in paths {
                println!("{}", p.display());
            }
            Ok(())
        }
        Command::Disks => disks(),
        Command::Flash {
            disk,
            dir,
            image,
            sha256,
            no_verify,
            no_boot_entry,
            dry_run,
            yes,
        } => flash(&FlashArgs {
            disk,
            dir,
            image,
            sha256,
            verify: !no_verify,
            boot_entry: !no_boot_entry,
            dry_run,
            yes,
        }),
        Command::BootEntry { image, name } => {
            let head = Image::head(&[image], partition::HEAD_BYTES)?;
            let esp = partition::find_efi_partition(&head)?;
            println!("{esp:?}");
            let option = boot_entry::load_option(&name, &esp, boot_entry::BOOT_FILE);
            println!("{}", hex(&option));
            Ok(())
        }
    }
}

fn hex(data: &[u8]) -> String {
    data.chunks(16)
        .map(|line| {
            line.iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(clippy::struct_excessive_bools, dead_code)]
struct FlashArgs {
    disk: u32,
    dir: Option<PathBuf>,
    image: Option<PathBuf>,
    sha256: Option<String>,
    verify: bool,
    boot_entry: bool,
    dry_run: bool,
    yes: bool,
}

#[cfg(windows)]
fn disks() -> bootstick_core::Result<()> {
    for d in bootstick_core::windows::disks::list()? {
        let verdict = match d.check_writable(0) {
            Ok(()) => "can be written".to_owned(),
            Err(e) => e.to_string(),
        };
        println!(
            "{:3}  {}  [{:?}, {} B sectors]  {verdict}",
            d.number,
            d.label(),
            d.bus,
            d.sector_size
        );
    }
    Ok(())
}

#[cfg(not(windows))]
fn disks() -> bootstick_core::Result<()> {
    Err(bootstick_core::Error::Refused(
        "listing disks only works on Windows".to_owned(),
    ))
}

#[cfg(windows)]
fn flash(args: &FlashArgs) -> bootstick_core::Result<()> {
    use bootstick_core::windows::{Flash, flash};

    let (paths, sha256, name) = if let (Some(image), Some(sha)) = (&args.image, &args.sha256) {
        (vec![image.clone()], sha.clone(), "Installer".to_owned())
    } else {
        let agent = release::agent();
        let m = Manifest::fetch(&agent)?;
        let dir = args.dir.clone().unwrap_or_else(|| download::cache_dir(&m));
        let paths = download::download_release(&agent, &m, &dir, &mut Console { last: None })?;
        (paths, m.iso.sha256.clone(), format!("{} installer", m.name))
    };

    let mut job = Flash {
        disk: args.disk,
        image: &paths,
        sha256: &sha256,
        verify: args.verify,
        boot_entry: args.boot_entry.then_some(name.as_str()),
        dry_run: true,
    };
    let check = flash(&job, &mut Console { last: None })?;
    println!("Disk: {} ({} bytes to write)", check.disk, check.bytes);
    if args.dry_run {
        println!("Dry run: nothing was written.");
        return Ok(());
    }
    if !args.yes && !confirm(&check.disk)? {
        println!("Cancelled. Nothing was changed.");
        return Ok(());
    }
    job.dry_run = false;
    let report = flash(&job, &mut Console { last: None })?;
    println!("Done: {}", report.disk);
    match (report.boot_entry, report.boot_entry_skipped) {
        (Some(n), _) => println!(
            "The next start boots from the stick ({}).",
            boot_entry::variable_name(n)
        ),
        (None, Some(why)) => println!("No boot entry: {why}. Choose the stick in the boot menu."),
        (None, None) => {}
    }
    Ok(())
}

#[cfg(not(windows))]
fn flash(_: &FlashArgs) -> bootstick_core::Result<()> {
    Err(bootstick_core::Error::Refused(
        "writing sticks only works on Windows".to_owned(),
    ))
}

#[cfg(windows)]
fn confirm(disk: &str) -> bootstick_core::Result<bool> {
    print!("Everything on {disk} will be deleted. Type yes to continue: ");
    io::stdout().flush().ok();
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|source| bootstick_core::Error::Io {
            context: "read the answer".to_owned(),
            source,
        })?;
    Ok(answer.trim().eq_ignore_ascii_case("yes"))
}
