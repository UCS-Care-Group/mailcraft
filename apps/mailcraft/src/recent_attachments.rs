// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The newest available local documents from Windows Recent Items.

use std::collections::{BinaryHeap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf, Prefix};

use encoding_rs::Encoding;

const MOST_FILES: usize = 10;
const MOST_LINKS: usize = 256;
const MOST_LINK_BYTES: u64 = 64 * 1024;
const MOST_PATH_UNITS: usize = 1024;
const REPARSE_POINT: u32 = 0x400;
const OFFLINE: u32 = 0x1000;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;

#[cfg(test)]
mod tests;

pub(crate) fn list() -> Vec<PathBuf> {
    let Some(appdata) = std::env::var_os("APPDATA") else { return Vec::new() };
    let recent = PathBuf::from(appdata).join(r"Microsoft\Windows\Recent");
    let fixed = fixed_drives();
    from_directory(&recent, &fixed, ansi_encoding())
}

/// Determine the drive letters of local fixed volumes before probing a shortcut target.
fn fixed_drives() -> HashSet<u8> {
    use sysinfo::{DiskRefreshKind, Disks};

    let Ok(disks) = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing()) else { return HashSet::new() };
    disks
        .iter()
        .filter(|disk| !disk.is_removable())
        .filter_map(|disk| {
            let mut parts = disk.mount_point().components();
            let Component::Prefix(prefix) = parts.next()? else { return None };
            let Prefix::Disk(drive) = prefix.kind() else { return None };
            if parts.next() != Some(Component::RootDir) || parts.next().is_some() {
                return None;
            }
            Some(drive.to_ascii_uppercase())
        })
        .collect()
}

fn ansi_encoding() -> Option<&'static Encoding> {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;

    let nls = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"SYSTEM\CurrentControlSet\Control\Nls\CodePage").ok()?;
    let code = nls.get_value::<String, _>("ACP").ok()?.parse::<u32>().ok()?;
    let label = match code {
        874 | 1250..=1258 => format!("windows-{code}"),
        932 => "shift_jis".to_owned(),
        936 => "gbk".to_owned(),
        949 => "euc-kr".to_owned(),
        950 => "big5".to_owned(),
        65001 => "utf-8".to_owned(),
        _ => return None,
    };
    Encoding::for_label_no_replacement(label.as_bytes())
}

fn from_directory(directory: &Path, fixed: &HashSet<u8>, ansi: Option<&'static Encoding>) -> Vec<PathBuf> {
    if !local_path(directory, fixed) {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(directory) else { return Vec::new() };
    // Keep the newest candidates in bounded space, including when many entries are stale.
    let mut newest = BinaryHeap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else { continue };
        if !metadata.is_file() || metadata.len() > MOST_LINK_BYTES || metadata.file_attributes() & (REPARSE_POINT | OFFLINE) != 0 {
            continue;
        }
        let Ok(modified) = metadata.modified() else { continue };
        newest.push(std::cmp::Reverse((modified, path)));
        if newest.len() > MOST_LINKS {
            newest.pop();
        }
    }
    let mut targets = Vec::new();
    for std::cmp::Reverse((_, shortcut)) in newest.into_sorted_vec() {
        let Some(target) = read_shortcut(&shortcut, ansi) else { continue };
        if !local_path(&target, fixed) || !target.is_file() {
            continue;
        }
        // Windows paths normally compare case insensitively. Also collapse exact duplicates.
        if targets.iter().any(|other: &PathBuf| other.as_os_str().eq_ignore_ascii_case(target.as_os_str())) {
            continue;
        }
        targets.push(target);
        if targets.len() == MOST_FILES {
            break;
        }
    }
    targets
}

/// Only normal drive paths, without reparse points in the parent chain or at the file.
fn local_path(path: &Path, fixed: &HashSet<u8>) -> bool {
    let mut parts = path.components();
    let Some(Component::Prefix(prefix)) = parts.next() else { return false };
    if !matches!(prefix.kind(), Prefix::Disk(drive) if fixed.contains(&drive.to_ascii_uppercase())) || parts.next() != Some(Component::RootDir) {
        return false;
    }
    let mut checked = PathBuf::from(prefix.as_os_str());
    checked.push(r"\");
    for component in parts {
        let Component::Normal(name) = component else { return false };
        let Some(name_text) = name.to_str() else { return false };
        if name_text.contains([':', '/', '\\']) || name_text.ends_with([' ', '.']) {
            return false;
        }
        checked.push(name);
        let Ok(meta) = fs::symlink_metadata(&checked) else { return false };
        if meta.file_attributes() & (REPARSE_POINT | OFFLINE) != 0 {
            return false;
        }
    }
    true
}

fn read_shortcut(path: &Path, ansi: Option<&'static Encoding>) -> Option<PathBuf> {
    // Inspect the opened handle and cap actual reads too, in case the file grows.
    let mut file = File::options().read(true).custom_flags(OPEN_REPARSE_POINT).open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MOST_LINK_BYTES || meta.file_attributes() & (REPARSE_POINT | OFFLINE) != 0 {
        return None;
    }
    let mut bytes = Vec::new();
    (&mut file).take(MOST_LINK_BYTES).read_to_end(&mut bytes).ok()?;
    if file.metadata().ok()?.len() != bytes.len() as u64 {
        return None;
    }
    link_target(&bytes, ansi)
}

fn field(bytes: &[u8], at: usize) -> Option<usize> {
    let word = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
    usize::try_from(u32::from_le_bytes(word)).ok()
}

/// A local file link with a fixed-drive LinkInfo, as specified by MS-SHLLINK §§2.1–2.3.
/// IDLists are skipped. Network, relative, environment-variable and executable link data
/// aren't resolved, and no Shell code is invoked on an untrusted shortcut.
fn link_target(bytes: &[u8], ansi: Option<&'static Encoding>) -> Option<PathBuf> {
    const HEADER: usize = 0x4c;
    const CLSID: [u8; 16] = [0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];
    if field(bytes, 0)? != HEADER || bytes.get(4..20)? != CLSID {
        return None;
    }
    let flags = field(bytes, 20)?;
    if flags & 2 == 0 || flags & 0x100 != 0 {
        return None;
    }
    if field(bytes, 24)? & 0x10 != 0 {
        // FILE_ATTRIBUTE_DIRECTORY
        return None;
    }
    let mut info = bytes.get(HEADER..)?;
    if flags & 1 != 0 {
        let size = u16::from_le_bytes(info.get(..2)?.try_into().ok()?);
        info = info.get(2 + usize::from(size)..)?;
    }
    let info = info.get(..field(info, 0)?)?;
    let header = field(info, 4)?;
    if (header != 0x1c && header < 0x24) || header > info.len() || field(info, 8)? & 1 == 0 {
        return None;
    }
    let offset = |at| field(info, at).filter(|offset| *offset >= header && *offset < info.len());
    let volume = info.get(offset(12)?..)?;
    let volume = volume.get(..field(volume, 0)?)?;
    if field(volume, 4)? != 3 {
        return None;
    }
    let string = |at, wide| {
        let bytes = info.get(offset(at)?..)?;
        if wide {
            let mut units = Vec::new();
            for pair in bytes.chunks_exact(2).take(MOST_PATH_UNITS + 1) {
                let unit = u16::from_le_bytes(pair.try_into().ok()?);
                if unit == 0 {
                    return String::from_utf16(&units).ok();
                }
                units.push(unit);
            }
            None
        } else {
            // Unicode offsets take precedence. Decode a legacy path in the system code page.
            let end = bytes.iter().take(4 * MOST_PATH_UNITS + 1).position(|byte| *byte == 0)?;
            let encoded = bytes.get(..end)?;
            if encoded.is_ascii() {
                return std::str::from_utf8(encoded).ok().map(str::to_owned);
            }
            ansi?.decode_without_bom_handling_and_without_replacement(encoded).map(std::borrow::Cow::into_owned)
        }
    };
    let base = if header >= 0x24 && field(info, 28)? != 0 { string(28, true)? } else { string(16, false)? };
    let suffix = if header >= 0x24 && field(info, 32)? != 0 { string(32, true)? } else { string(24, false)? };
    let target = format!("{base}{suffix}");
    if target.encode_utf16().count() > MOST_PATH_UNITS || target.chars().any(char::is_control) {
        return None;
    }
    let target = PathBuf::from(target);
    if target.extension().and_then(|e| e.to_str()).is_some_and(|ext| ["lnk", "url", "website"].iter().any(|kind| ext.eq_ignore_ascii_case(kind))) {
        return None;
    }
    Some(target)
}
