//! Finding the EFI system partition in the image, for the boot entry.
//!
//! The NixOS ISO is a hybrid image: an MBR with an ISO 9660 partition and a
//! small EFI system partition (type 0xEF), plus a GPT header. The MBR has no
//! protective 0xEE entry, so UEFI firmware treats the stick as an MBR disk and
//! the boot entry must describe the MBR partition. An image with a protective
//! MBR is read as GPT instead.

use crate::{Error, Result};

/// The sector size of the partition tables in an ISO image
pub const IMAGE_SECTOR: u64 = 512;

/// Bytes of the image needed to read both tables
pub const HEAD_BYTES: usize = 64 * 1024;

const ESP_GUID: [u8; 16] = [
    0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signature {
    /// The 32-bit disk signature of the MBR
    Mbr(u32),
    /// The unique GUID of the GPT partition, as stored on disk
    Gpt([u8; 16]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EfiPartition {
    /// 1-based, as in the UEFI device path
    pub number: u32,
    pub start_lba: u64,
    pub size_lba: u64,
    pub signature: Signature,
}

/// Finds the EFI system partition in the first bytes of an image
pub fn find_efi_partition(head: &[u8]) -> Result<EfiPartition> {
    if head.len() < 512 || head[510..512] != [0x55, 0xAA] {
        return Err(Error::NoEfiPartition);
    }
    let entries: Vec<&[u8]> = (0..4)
        .map(|i| &head[446 + 16 * i..446 + 16 * (i + 1)])
        .collect();
    let protective = entries.iter().any(|e| e[4] == 0xEE);
    if protective {
        find_gpt(head)
    } else {
        let signature = u32_at(head, 440);
        entries
            .iter()
            .enumerate()
            .find(|(_, e)| e[4] == 0xEF)
            .map(|(i, e)| EfiPartition {
                number: i as u32 + 1,
                start_lba: u64::from(u32_at(e, 8)),
                size_lba: u64::from(u32_at(e, 12)),
                signature: Signature::Mbr(signature),
            })
            .ok_or(Error::NoEfiPartition)
    }
}

fn find_gpt(head: &[u8]) -> Result<EfiPartition> {
    let header = head.get(512..512 + 92).ok_or(Error::NoEfiPartition)?;
    if &header[..8] != b"EFI PART" {
        return Err(Error::NoEfiPartition);
    }
    let entries_lba = u64_at(header, 72);
    let count = u32_at(header, 80);
    let entry_size = u32_at(header, 84) as usize;
    if entry_size < 128 {
        return Err(Error::NoEfiPartition);
    }
    let start = usize::try_from(entries_lba * IMAGE_SECTOR).map_err(|_| Error::NoEfiPartition)?;
    for i in 0..count as usize {
        let Some(entry) = head.get(start + i * entry_size..start + (i + 1) * entry_size) else {
            break;
        };
        if entry[..16] == ESP_GUID {
            let first = u64_at(entry, 32);
            let last = u64_at(entry, 40);
            let mut guid = [0; 16];
            guid.copy_from_slice(&entry[16..32]);
            return Ok(EfiPartition {
                number: i as u32 + 1,
                start_lba: first,
                size_lba: last - first + 1,
                signature: Signature::Gpt(guid),
            });
        }
    }
    Err(Error::NoEfiPartition)
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap_or_default())
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MBR of strix-0.1.0-x86_64.iso (first 512 bytes, partition part only)
    fn strix_010_mbr() -> Vec<u8> {
        let mut head = vec![0u8; 1024];
        head[440..444].copy_from_slice(&[0x2f, 0x38, 0xb6, 0x9f]);
        // 1: ISO 9660, type 0x00, bootable
        head[446] = 0x80;
        head[446 + 8..446 + 12].copy_from_slice(&0u32.to_le_bytes());
        head[446 + 12..446 + 16].copy_from_slice(&3_150_560u32.to_le_bytes());
        // 2: EFI system partition
        head[462 + 4] = 0xEF;
        head[462 + 8..462 + 12].copy_from_slice(&268u32.to_le_bytes());
        head[462 + 12..462 + 16].copy_from_slice(&6144u32.to_le_bytes());
        head[510] = 0x55;
        head[511] = 0xAA;
        // The GPT header is there too, but the MBR is not protective
        head[512..520].copy_from_slice(b"EFI PART");
        head
    }

    #[test]
    fn hybrid_iso_uses_the_mbr() {
        let p = find_efi_partition(&strix_010_mbr()).unwrap();
        assert_eq!(
            p,
            EfiPartition {
                number: 2,
                start_lba: 268,
                size_lba: 6144,
                signature: Signature::Mbr(0x9fb6_382f),
            }
        );
    }

    #[test]
    fn protective_mbr_uses_gpt() {
        let mut head = vec![0u8; HEAD_BYTES];
        head[446 + 4] = 0xEE;
        head[510] = 0x55;
        head[511] = 0xAA;
        head[512..520].copy_from_slice(b"EFI PART");
        head[512 + 72..512 + 80].copy_from_slice(&2u64.to_le_bytes());
        head[512 + 80..512 + 84].copy_from_slice(&128u32.to_le_bytes());
        head[512 + 84..512 + 88].copy_from_slice(&128u32.to_le_bytes());
        // Second entry is the ESP
        let e = 1024 + 128;
        head[e..e + 16].copy_from_slice(&ESP_GUID);
        head[e + 16..e + 32].copy_from_slice(&[9; 16]);
        head[e + 32..e + 40].copy_from_slice(&2048u64.to_le_bytes());
        head[e + 40..e + 48].copy_from_slice(&4095u64.to_le_bytes());
        let p = find_efi_partition(&head).unwrap();
        assert_eq!(p.number, 2);
        assert_eq!(p.start_lba, 2048);
        assert_eq!(p.size_lba, 2048);
        assert_eq!(p.signature, Signature::Gpt([9; 16]));
    }

    #[test]
    fn no_esp() {
        let mut head = strix_010_mbr();
        head[462 + 4] = 0x0C;
        assert!(find_efi_partition(&head).is_err());
        assert!(find_efi_partition(&[0; 100]).is_err());
    }
}
