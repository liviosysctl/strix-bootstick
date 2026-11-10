//! Downloading the release files with a SHA256 check per file.
//!
//! Every file goes to `<name>.partial` first and only gets its real name once
//! size and SHA256 match. A file that is already there with the right SHA256
//! is not downloaded again, so a second stick or a retry after a broken
//! connection costs nothing. A file with a wrong SHA256 is downloaded once
//! more before giving up.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::manifest::{Download, Manifest};
use crate::progress::{Observer, Progress, Stage, check_cancel};
use crate::{Error, IoContext, Result};

const ATTEMPTS: u32 = 2;
const BUFFER: usize = 1 << 20;

/// Downloads every file of the manifest into `dir` and returns their paths in
/// order. Together they are the ISO.
pub fn download_release(
    agent: &ureq::Agent,
    manifest: &Manifest,
    dir: &Path,
    observer: &mut dyn Observer,
) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(dir).context(|| format!("create {}", dir.display()))?;
    let downloads = manifest.downloads();
    let total: u64 = downloads.iter().map(|d| d.size).sum();
    let mut done_before = 0;
    let mut paths = Vec::new();
    for download in &downloads {
        let path = dir.join(&download.file);
        download_file(agent, download, &path, done_before, total, observer)?;
        done_before += download.size;
        paths.push(path);
    }
    Ok(paths)
}

fn download_file(
    agent: &ureq::Agent,
    download: &Download,
    path: &Path,
    done_before: u64,
    total: u64,
    observer: &mut dyn Observer,
) -> Result<()> {
    if path.exists() && file_matches(path, download)? {
        observer.progress(&Progress {
            stage: Stage::Download,
            done: done_before + download.size,
            total,
            file: download.file.clone(),
        });
        return Ok(());
    }

    let partial = path.with_extension(format!(
        "{}partial",
        path.extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    let mut last_error = None;
    for _ in 0..ATTEMPTS {
        check_cancel(observer)?;
        match fetch(agent, download, &partial, done_before, total, observer) {
            Ok(()) => {
                fs::rename(&partial, path).context(|| format!("rename to {}", path.display()))?;
                return Ok(());
            }
            Err(Error::Cancelled) => {
                let _ = fs::remove_file(&partial);
                return Err(Error::Cancelled);
            }
            Err(error) => {
                let _ = fs::remove_file(&partial);
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or(Error::Cancelled))
}

fn fetch(
    agent: &ureq::Agent,
    download: &Download,
    partial: &Path,
    done_before: u64,
    total: u64,
    observer: &mut dyn Observer,
) -> Result<()> {
    let mut response = agent.get(&download.url).call()?;
    let mut body = response
        .body_mut()
        .with_config()
        .limit(download.size.saturating_add(1))
        .reader();
    let mut file = File::create(partial).context(|| format!("create {}", partial.display()))?;
    copy_hashed(
        &mut body,
        &mut file,
        download,
        |done| Progress {
            stage: Stage::Download,
            done: done_before + done,
            total,
            file: download.file.clone(),
        },
        observer,
    )?;
    file.sync_all()
        .context(|| format!("write {}", partial.display()))
}

/// Copies and hashes at the same time, then checks size and SHA256
pub(crate) fn copy_hashed(
    from: &mut dyn Read,
    to: &mut dyn Write,
    expected: &Download,
    report: impl Fn(u64) -> Progress,
    observer: &mut dyn Observer,
) -> Result<()> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; BUFFER];
    let mut done: u64 = 0;
    loop {
        check_cancel(observer)?;
        let n = match from.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(Error::Io {
                    context: format!("download {}", expected.file),
                    source: e,
                });
            }
        };
        hasher.update(&buffer[..n]);
        to.write_all(&buffer[..n])
            .context(|| format!("save {}", expected.file))?;
        done += n as u64;
        observer.progress(&report(done));
    }
    if done != expected.size {
        return Err(Error::Size {
            file: expected.file.clone(),
            expected: expected.size,
            actual: done,
        });
    }
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(&expected.sha256) {
        return Err(Error::Checksum {
            file: expected.file.clone(),
            expected: expected.sha256.clone(),
            actual,
        });
    }
    Ok(())
}

fn file_matches(path: &Path, download: &Download) -> Result<bool> {
    let size = fs::metadata(path)
        .context(|| format!("read {}", path.display()))?
        .len();
    if size != download.size {
        return Ok(false);
    }
    let actual = sha256_file(path)?;
    Ok(actual.eq_ignore_ascii_case(&download.sha256))
}

/// SHA256 of a file as lowercase hex
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; BUFFER];
    loop {
        let n = file
            .read(&mut buffer)
            .context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::Silent;

    fn download_for(data: &[u8]) -> Download {
        Download {
            file: "test.iso".into(),
            url: String::new(),
            size: data.len() as u64,
            sha256: hex::encode(Sha256::digest(data)),
        }
    }

    fn progress(done: u64) -> Progress {
        Progress {
            stage: Stage::Download,
            done,
            total: 0,
            file: String::new(),
        }
    }

    #[test]
    fn copy_checks_hash_and_size() {
        let data = vec![7u8; 3 * BUFFER + 5];
        let mut out = Vec::new();
        copy_hashed(
            &mut data.as_slice(),
            &mut out,
            &download_for(&data),
            progress,
            &mut Silent,
        )
        .unwrap();
        assert_eq!(out, data);

        let mut wrong = download_for(&data);
        wrong.sha256 = "0".repeat(64);
        assert!(matches!(
            copy_hashed(
                &mut data.as_slice(),
                &mut Vec::new(),
                &wrong,
                progress,
                &mut Silent
            ),
            Err(Error::Checksum { .. })
        ));

        let short = &data[..10];
        assert!(matches!(
            copy_hashed(
                &mut &short[..],
                &mut Vec::new(),
                &download_for(&data),
                progress,
                &mut Silent
            ),
            Err(Error::Size { .. })
        ));
    }

    #[test]
    fn existing_file_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"strix".to_vec();
        let path = dir.path().join("test.iso");
        fs::write(&path, &data).unwrap();
        assert!(file_matches(&path, &download_for(&data)).unwrap());
        fs::write(&path, b"other").unwrap();
        assert!(!file_matches(&path, &download_for(&data)).unwrap());
    }
}
