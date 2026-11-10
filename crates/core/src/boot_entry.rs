//! A UEFI boot entry for the stick, in the binary form the firmware stores in
//! the variables `Boot####` (UEFI spec, "Load Options" and "Device Path
//! Protocol"). Writing the variables is the Windows part
//! ([`crate::windows::firmware`]); everything here is plain bytes.

use crate::partition::{EfiPartition, Signature};

/// The file the firmware starts on a removable medium, and that the NixOS ISO
/// carries in its EFI system partition
pub const BOOT_FILE: &str = r"\EFI\BOOT\BOOTX64.EFI";

const LOAD_OPTION_ACTIVE: u32 = 0x1;

/// An `EFI_LOAD_OPTION`: attributes, length of the device path, description,
/// device path (hard drive partition, file, end)
#[must_use]
pub fn load_option(description: &str, partition: &EfiPartition, file: &str) -> Vec<u8> {
    let path = device_path(partition, file);
    let mut out = Vec::new();
    out.extend_from_slice(&LOAD_OPTION_ACTIVE.to_le_bytes());
    out.extend_from_slice(&u16::try_from(path.len()).unwrap_or(u16::MAX).to_le_bytes());
    out.extend_from_slice(&utf16z(description));
    out.extend_from_slice(&path);
    out
}

fn device_path(partition: &EfiPartition, file: &str) -> Vec<u8> {
    let mut out = Vec::new();

    // Media device path, hard drive (type 4, subtype 1), always 42 bytes
    out.extend_from_slice(&[0x04, 0x01]);
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&partition.number.to_le_bytes());
    out.extend_from_slice(&partition.start_lba.to_le_bytes());
    out.extend_from_slice(&partition.size_lba.to_le_bytes());
    let (signature, format, kind) = match partition.signature {
        Signature::Mbr(sig) => {
            let mut s = [0u8; 16];
            s[..4].copy_from_slice(&sig.to_le_bytes());
            (s, 0x01u8, 0x01u8)
        }
        Signature::Gpt(guid) => (guid, 0x02, 0x02),
    };
    out.extend_from_slice(&signature);
    out.push(format);
    out.push(kind);

    // Media device path, file path (type 4, subtype 4)
    let name = utf16z(file);
    out.extend_from_slice(&[0x04, 0x04]);
    out.extend_from_slice(
        &u16::try_from(4 + name.len())
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(&name);

    // End of the whole device path
    out.extend_from_slice(&[0x7F, 0xFF, 0x04, 0x00]);
    out
}

/// The description of a stored load option, to find an entry this app made
/// before and reuse its number
#[must_use]
pub fn description(option: &[u8]) -> Option<String> {
    let text = option.get(6..)?;
    let units: Vec<u16> = text
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16(&units).ok()
}

/// The variable name of a boot entry
#[must_use]
pub fn variable_name(number: u16) -> String {
    format!("Boot{number:04X}")
}

/// The number for our entry: the one that already carries our description,
/// otherwise the lowest free one. `existing` lists the numbers in use with
/// their descriptions.
#[must_use]
pub fn pick_number(existing: &[(u16, Option<String>)], ours: &str) -> Option<u16> {
    if let Some((n, _)) = existing.iter().find(|(_, d)| d.as_deref() == Some(ours)) {
        return Some(*n);
    }
    (0..=u16::MAX).find(|n| !existing.iter().any(|(e, _)| e == n))
}

fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strix_esp() -> EfiPartition {
        EfiPartition {
            number: 2,
            start_lba: 268,
            size_lba: 6144,
            signature: Signature::Mbr(0x9fb6_382f),
        }
    }

    #[test]
    fn load_option_layout() {
        let option = load_option("Strix installer", &strix_esp(), BOOT_FILE);
        // Attributes: active
        assert_eq!(&option[..4], &[1, 0, 0, 0]);
        let path_len = usize::from(u16::from_le_bytes([option[4], option[5]]));
        let desc_len = ("Strix installer".len() + 1) * 2;
        assert_eq!(option.len(), 6 + desc_len + path_len);
        assert_eq!(description(&option).as_deref(), Some("Strix installer"));

        let path = &option[6 + desc_len..];
        // Hard drive node
        assert_eq!(&path[..4], &[0x04, 0x01, 42, 0]);
        assert_eq!(u32::from_le_bytes(path[4..8].try_into().unwrap()), 2);
        assert_eq!(u64::from_le_bytes(path[8..16].try_into().unwrap()), 268);
        assert_eq!(u64::from_le_bytes(path[16..24].try_into().unwrap()), 6144);
        assert_eq!(&path[24..28], &[0x2f, 0x38, 0xb6, 0x9f]);
        assert_eq!(&path[28..40], &[0; 12]);
        assert_eq!(&path[40..42], &[0x01, 0x01]);
        // File node with the path in UTF-16 and a terminating zero
        let file_len = 4 + (BOOT_FILE.len() + 1) * 2;
        assert_eq!(&path[42..44], &[0x04, 0x04]);
        assert_eq!(
            usize::from(u16::from_le_bytes([path[44], path[45]])),
            file_len
        );
        // End node closes the path
        assert_eq!(&path[path.len() - 4..], &[0x7F, 0xFF, 0x04, 0x00]);
        assert_eq!(path.len(), 42 + file_len + 4);
    }

    #[test]
    fn gpt_signature() {
        let esp = EfiPartition {
            signature: Signature::Gpt([7; 16]),
            ..strix_esp()
        };
        let option = load_option("x", &esp, BOOT_FILE);
        let path = &option[6 + 4..];
        assert_eq!(&path[24..40], &[7; 16]);
        assert_eq!(&path[40..42], &[0x02, 0x02]);
    }

    #[test]
    fn numbers() {
        assert_eq!(variable_name(10), "Boot000A");
        let existing = vec![
            (0, Some("Windows Boot Manager".to_owned())),
            (1, None),
            (3, Some("Strix installer".to_owned())),
        ];
        assert_eq!(pick_number(&existing, "Strix installer"), Some(3));
        assert_eq!(pick_number(&existing, "Other"), Some(2));
    }
}
