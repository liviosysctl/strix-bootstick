# strix-bootstick

The Windows app that turns a USB stick into a Strix installer: plug in a stick, start the app, done. Nobody has to see partitions, Rufus or a firmware menu. Part of the M245 proof of concept; the distribution itself lives in [liviosysctl/StrixOS](https://github.com/liviosysctl/StrixOS).

The app has no distro name built in. It shows the name from the release's `manifest.json`.

## What it does

1. Fetches `manifest.json` of the proof of concept's ISO (`releases/download/poc/manifest.json` of StrixOS: a pre-release that CI on the branch `m245-poc` renews on every push; no GitHub API, no token) and checks it: format version, file names, SHA256 values, sizes.
2. Downloads the ISO, or its parts when it is split below 2 GiB, and checks the SHA256 of every file. A file that is already downloaded and correct is reused, so a second stick costs no download.
3. Lists the disks and only offers USB sticks and SD cards: never the disk Windows runs from, never anything larger than 256 GB (probably a backup drive). The check runs again right before writing.
4. Writes the image raw, like Rufus in DD mode:
   - lock and dismount every volume on the stick, retried for a few seconds;
   - open `\\.\PhysicalDriveN` unbuffered and write-through, delete the partition layout Windows knows;
   - wipe the first and the last MiB (an old GPT backup header at the end would confuse firmware and Windows);
   - write in 4 MiB blocks aligned to the sector size, each failed block retried;
   - flush, let Windows read the new partition table.
5. Reads the whole stick back and compares the SHA256 with the manifest. Rufus does not do this by default; a stick that fails half way through an install is the worst outcome in a demo.
6. Creates a firmware boot entry for the stick's EFI partition and sets `BootNext` to it, so the next start boots from the stick once, without the firmware menu. `BootOrder` stays as it is, Windows remains the normal start. Skipped on non-UEFI machines and sticks with sectors other than 512 bytes.

## The app

`strix-bootstick.exe`, one file, nothing to install. Windows asks for administrator rights once at the start (writing a whole disk and the firmware entry need them), then there is one window:

1. It looks for the newest version and waits for a stick. Only USB sticks and SD cards are listed; one that is too small or looks like a backup drive is shown but cannot be chosen. With a single usable stick that one is already chosen.
2. "Create installer stick", then one question: everything on the stick will be deleted.
3. Download, write, check, each with its progress, speed and time left. Cancel asks first, and so does closing the window. The PC does not go to sleep meanwhile.
4. "The stick is ready." If the firmware took the boot entry: "Restart now", and the PC starts from the stick once. If not, the app says to pick the stick in the boot menu and shows the usual keys.

The window is in English or German, following the language of Windows. It looks like the rest of Strix: the page is built from [StrixUI](https://github.com/liviosysctl/StrixUI) (tokens, components, fonts, the Saturnus plate), copied into `crates/app/ui/strixui/`.

## Layout

| Path | What |
|---|---|
| `crates/core` | Everything under "What it does" as a library. The Windows parts are in `src/windows/`, the rest is plain Rust and tested on any system. |
| `crates/app` | `strix-bootstick`, the window: [Tauri](https://tauri.app) 2, so the back end is Rust and the page runs in the WebView2 that Windows 10 and 11 bring along. `src/window.rs` has the commands and events between the two, `src/wire.rs` their JSON. |
| `crates/app/ui` | The page: `index.html`, `app.css`, `app.js`, `strings.js` (every text, English and German), and `strixui/` (copied from StrixUI, never edited here; its README says from which commit and how). No npm, no bundler: the files are embedded in the `.exe` as they are. |
| `crates/cli` | `bootstick`, the core on the command line, to test a step on its own on a real machine. |

## Command line

On Windows, in a terminal started as administrator:

```
bootstick manifest                         # the current ISO of the proof of concept
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
cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu   # the Windows code, from Linux (needs gcc-mingw-w64-x86-64 for the app's icon and manifest)
cargo build --release -p bootstick-app                                  # on Windows: target/release/strix-bootstick.exe
```

The window only exists on Windows; on Linux `crates/app` compiles to a stub with the tests of its JSON, so the workspace builds everywhere. A plain `cargo build` gives the finished app, the Tauri command line is not needed.

CI runs the tests on Linux and Windows. The Windows run then starts what it built on the runner: the command line reads the real release and lists the runner's disks, and the app has to open its window (a screenshot of it is kept). `strix-bootstick.exe` and `bootstick.exe` of every push are an artifact of that run. A version tag attaches `strix-bootstick.exe` to its GitHub release.

A third job, `stick`, goes the whole way on a real Windows on every push: it downloads the release, writes it, reads it back and sets the boot entry. The runner has no USB port, so an attached virtual disk with a volume on it stands in for the stick. Only that one build accepts it, through the feature `virtual-disks` of the core; the programs that are handed out never do. Commit and release rules are in [CONTRIBUTING.md](CONTRIBUTING.md).

Always the newest versions: CI updates Rust to the newest stable before every run, dependencies are kept at their newest releases (`cargo update`, and new major versions are taken over by hand).
