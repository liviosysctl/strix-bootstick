# strix-bootstick

The Windows app that turns a USB stick into a Strix installer: plug in a stick, start the app, done. Nobody has to see partitions, Rufus or a firmware menu. Part of the M245 proof of concept; the distribution itself lives in [liviosysctl/StrixOS](https://github.com/liviosysctl/StrixOS).

The app has no distro name built in. It shows the name from the release's `manifest.json`.

## What it does

1. Fetches `manifest.json` of the newest release of StrixOS (`releases/latest/download/manifest.json`, no GitHub API, no token) and checks it: format version, file names, SHA256 values, sizes.
2. Downloads the ISO, or its parts when the release splits it below 2 GiB, and checks the SHA256 of every file. A file that is already downloaded and correct is reused, so a second stick costs no download.
3. Lists the disks and only offers USB sticks and SD cards: never the disk Windows runs from, never anything larger than 256 GB (probably a backup drive). The check runs again right before writing.
4. Writes the image raw, like Rufus in DD mode:
   - lock and dismount every volume on the stick, retried for a few seconds;
   - open `\\.\PhysicalDriveN` unbuffered and write-through, delete the partition layout Windows knows;
   - wipe the first and the last MiB (an old GPT backup header at the end would confuse firmware and Windows);
   - write in 4 MiB blocks aligned to the sector size, each failed block retried;
   - flush, let Windows read the new partition table.
5. Reads the whole stick back and compares the SHA256 with the manifest. Rufus does not do this by default; a stick that fails half way through an install is the worst outcome in a demo.
6. Creates a firmware boot entry for the stick's EFI partition and sets `BootNext` to it, so the next start boots from the stick once, without the firmware menu. `BootOrder` stays as it is, Windows remains the normal start. Skipped on non-UEFI machines and sticks with sectors other than 512 bytes.

## Layout

| Path | What |
|---|---|
| `crates/core` | Everything above as a library. The Windows parts are in `src/windows/`, the rest is plain Rust and tested on any system. |
| `crates/cli` | `bootstick`, the core on the command line, to test on real machines before the app window exists. |

The app window (Tauri) comes once the Strix design system is done.

## Command line

On Windows, in a terminal started as administrator:

```
bootstick manifest                         # the newest release
bootstick download                         # download and check, into %TEMP%\bootstick
bootstick disks                            # all disks, and which may be written
bootstick flash --disk 2 --dry-run         # check everything, write nothing
bootstick flash --disk 2                   # write, read back, set the boot entry
bootstick flash --disk 2 --no-boot-entry   # leave the firmware alone
```

`manifest`, `download` and `boot-entry --image <iso>` also work on Linux.

## Development

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu   # the Windows code, from Linux
```

CI runs the tests on Linux and Windows and keeps `bootstick.exe` from every push as an artifact. Commit and release rules are in [CONTRIBUTING.md](CONTRIBUTING.md).

Always the newest versions: CI updates Rust to the newest stable before every run, dependencies are kept at their newest releases (`cargo update`, and new major versions are taken over by hand).
