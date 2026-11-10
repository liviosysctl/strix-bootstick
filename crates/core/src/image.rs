//! The ISO as one stream, also when it was downloaded in parts

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::{IoContext, Result};

/// Reads the parts one after the other, as if they were one file
pub struct Image {
    paths: Vec<PathBuf>,
    index: usize,
    current: Option<File>,
    len: u64,
}

impl Image {
    pub fn open(paths: &[PathBuf]) -> Result<Self> {
        let mut len = 0;
        for path in paths {
            len += std::fs::metadata(path)
                .context(|| format!("read {}", path.display()))?
                .len();
        }
        Ok(Self {
            paths: paths.to_vec(),
            index: 0,
            current: None,
            len,
        })
    }

    #[must_use]
    pub fn len(&self) -> u64 {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The first bytes of the image, for the partition table
    pub fn head(paths: &[PathBuf], bytes: usize) -> Result<Vec<u8>> {
        let mut image = Self::open(paths)?;
        let mut head = Vec::with_capacity(bytes);
        (&mut image)
            .take(bytes as u64)
            .read_to_end(&mut head)
            .context(|| "read the start of the image".to_owned())?;
        Ok(head)
    }

    fn open_part(path: &Path) -> io::Result<File> {
        File::open(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
    }
}

impl Read for Image {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.current.is_none() {
                let Some(path) = self.paths.get(self.index) else {
                    return Ok(0);
                };
                self.current = Some(Self::open_part(path)?);
            }
            let n = self.current.as_mut().map_or(Ok(0), |f| f.read(buf))?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            self.current = None;
            self.index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_read_as_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for (i, data) in [&b"abc"[..], b"", b"defg"].iter().enumerate() {
            let p = dir.path().join(format!("part{i}"));
            std::fs::write(&p, data).unwrap();
            paths.push(p);
        }
        let mut image = Image::open(&paths).unwrap();
        assert_eq!(image.len(), 7);
        let mut all = Vec::new();
        image.read_to_end(&mut all).unwrap();
        assert_eq!(all, b"abcdefg");
        assert_eq!(Image::head(&paths, 4).unwrap(), b"abcd");
    }
}
