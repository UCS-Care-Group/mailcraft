// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

fn set(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn shortcut(target: &str) -> Vec<u8> {
    let mut bytes = vec![0; 0x4c];
    bytes[4..20].copy_from_slice(&[0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
    set(&mut bytes, 0, 0x4c);
    set(&mut bytes, 20, 2);
    let mut info = vec![0; 0x24 + 0x11];
    set(&mut info, 4, 0x24);
    set(&mut info, 8, 1);
    set(&mut info, 12, 0x24);
    set(&mut info, 0x24, 0x11);
    set(&mut info, 0x28, 3);
    set(&mut info, 0x30, 0x10);
    let base = info.len() as u32;
    set(&mut info, 16, base);
    set(&mut info, 24, base);
    info.push(0);
    set(&mut info, 28, base + 1);
    info.extend(target.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    let suffix = info.len() as u32;
    set(&mut info, 32, suffix);
    info.extend([0, 0]);
    let size = info.len() as u32;
    set(&mut info, 0, size);
    bytes.extend(info);
    bytes
}

#[test]
fn truncated_invalid_and_oversized_links_are_rejected() {
    let path = r"C:\Documents\bûche.txt";
    let good = shortcut(path);
    assert_eq!(link_target(&good, None), Some(PathBuf::from(path)));
    for size in 0..good.len() {
        assert!(link_target(&good[..size], None).is_none(), "{size}");
    }
    for (field, invalid) in [(0, 0), (20, 0x102), (24, 16), (0x4c, u32::MAX), (0x4c + 8, 3), (0x4c + 28, u32::MAX)] {
        let mut bytes = good.clone();
        set(&mut bytes, field, invalid);
        assert!(link_target(&bytes, None).is_none(), "{field}");
    }
    let temp = tempfile::tempdir().unwrap();
    let oversized = temp.path().join("oversized.lnk");
    let mut bytes = good;
    bytes.resize(MOST_LINK_BYTES as usize + 1, 0);
    fs::write(&oversized, bytes).unwrap();
    assert!(read_shortcut(&oversized, None).is_none());
}

#[test]
fn legacy_shortcuts_use_the_windows_ansi_code_page() {
    let mut bytes = shortcut(r"C:\Documents\bûche.txt");
    let info = &mut bytes[0x4c..];
    set(info, 28, 0);
    set(info, 32, 0);
    let base = bytes.len() - 0x4c;
    bytes.extend(b"C:\\Documents\\b\xfbche.txt\0\0");
    let size = bytes.len() - 0x4c;
    let info = &mut bytes[0x4c..];
    set(info, 0, size as u32);
    set(info, 16, base as u32);
    set(info, 24, size as u32 - 1);
    assert!(link_target(&bytes, None).is_none());
    assert_eq!(link_target(&bytes, Some(encoding_rs::WINDOWS_1252)), Some(PathBuf::from(r"C:\Documents\bûche.txt")));
}

#[test]
fn recent_list_is_bounded_and_ignores_duplicate_missing_and_unavailable_paths() {
    let temp = tempfile::tempdir().unwrap();
    let Prefix::Disk(drive) = (match temp.path().components().next() {
        Some(Component::Prefix(prefix)) => prefix.kind(),
        _ => panic!("temp folder needs a drive"),
    }) else {
        panic!("temp folder needs a normal drive")
    };
    let fixed = HashSet::from([drive.to_ascii_uppercase()]);
    for i in 0..12 {
        let target = temp.path().join(format!("{i}.txt"));
        fs::write(&target, b"synthetic fixture").unwrap();
        fs::write(temp.path().join(format!("{i}.lnk")), shortcut(target.to_str().unwrap())).unwrap();
        fs::write(temp.path().join(format!("duplicate-{i}.lnk")), shortcut(target.to_str().unwrap())).unwrap();
    }
    for (i, path) in [temp.path().join("missing.txt"), PathBuf::from(r"\\invalid\share\remote.txt"), temp.path().to_owned()].iter().enumerate() {
        fs::write(temp.path().join(format!("unavailable-{i}.lnk")), shortcut(path.to_str().unwrap())).unwrap();
    }
    let found = from_directory(temp.path(), &fixed, None);
    assert_eq!(found.len(), MOST_FILES);
    assert_eq!(found.iter().collect::<HashSet<_>>().len(), MOST_FILES);
    assert!(found.iter().all(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "txt")));
    assert!(from_directory(temp.path(), &HashSet::new(), None).is_empty());
}
