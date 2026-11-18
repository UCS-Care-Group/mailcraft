// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask version-code [X.Y.Z[-rc.N]]`: the Android version code for a version (the
//! workspace's when none is given), the one place it is worked out (`.github/workflows/android.yml`).
//!
//! Google Play takes each version code once and wants every upload's higher than the last, so each
//! version, release candidates included, has its own, and they rise in release order:
//! `MAJOR * 1_000_000 + MINOR * 10_000 + PATCH * 100 + N`, where N is the release candidate's
//! number (1 to 98) and 99 for the release itself. `1.2.3-rc.1` < `1.2.3-rc.2` < `1.2.3` <
//! `1.2.4-rc.1`. Minor and patch stay under 100 and major under 2100, which keeps every code under
//! Play's ceiling of 2,100,000,000. Anything else (another kind of prerelease, build metadata, a
//! number too big) is refused, never rounded into a code another version may take.

use std::path::Path;

/// Google Play's ceiling: a version code is at most this.
pub const PLAY_MAX: u32 = 2_100_000_000;

/// The suffix of the release itself; a release candidate's number stays below it.
const RELEASE: u32 = 99;

/// The Android version code of `version` (`X.Y.Z` or `X.Y.Z-rc.N`).
pub fn code(version: &str) -> Result<u32, String> {
    crate::version::validate(version)?;
    let bad = |why: &str| Err(format!("`{version}` has no Android version code: {why}"));
    let (core, pre) = match version.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (version, None),
    };
    let nums: Vec<u32> = match core.split('.').map(str::parse::<u32>).collect::<Result<Vec<_>, _>>() {
        Ok(n) => n,
        Err(_) => return bad("a number is too big"),
    };
    let [major, minor, patch] = nums.as_slice() else {
        return bad("not MAJOR.MINOR.PATCH");
    };
    if *major >= 2100 {
        return bad("the major version is 2100 or more");
    }
    if *minor > 99 || *patch > 99 {
        return bad("the minor or patch version is over 99");
    }
    let suffix = match pre {
        None => RELEASE,
        Some(p) => match p.strip_prefix("rc.").and_then(|n| n.parse::<u32>().ok()) {
            // `rc.7`, written plainly: not `rc.07` or `rc.+7`, which would share its code.
            Some(n) if (1..RELEASE).contains(&n) && p == format!("rc.{n}") => n,
            Some(_) => return bad("a release candidate's number is 1 to 98"),
            None => return bad("the only prerelease with one is -rc.N"),
        },
    };
    let code = major
        .checked_mul(1_000_000)
        .and_then(|c| c.checked_add(minor * 10_000))
        .and_then(|c| c.checked_add(patch * 100))
        .and_then(|c| c.checked_add(suffix));
    match code {
        Some(c) if c <= PLAY_MAX => Ok(c),
        _ => bad("over Google Play's ceiling"),
    }
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let version = match args {
        [] => {
            let path = root.join("Cargo.toml");
            let text = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
            crate::version::read(&text)?
        }
        [v] => (*v).to_string(),
        _ => return Err("usage: cargo xtask version-code [X.Y.Z[-rc.N]]".into()),
    };
    println!("{}", code(&version)?);
    Ok(())
}

#[cfg(test)]
#[path = "version_code_tests.rs"]
mod version_code_tests;
