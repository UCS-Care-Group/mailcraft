// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn problems(text: &str) -> Vec<String> {
    check("w.yml", text)
}

fn job(steps: &str) -> String {
    format!(
        "name: W\non: push\npermissions:\n  contents: read\njobs:\n  sign:\n    runs-on: ubuntu-24.04\n    steps:\n{steps}\n  other:\n    runs-on: ubuntu-24.04\n    steps:\n      - run: cargo build --locked\n"
    )
}

#[test]
fn the_repositorys_workflows_pass() {
    let root = crate::root();
    if let Err(e) = run(&root) {
        panic!("{e}");
    }
}

/// The publish job as it was: xtask compiled in the job that then signs with the release key.
#[test]
fn a_signing_job_that_compiles_its_tool_fails() {
    let text = job(
        "      - run: cargo build --locked --release --package xtask\n      - name: Sign the checksums\n        env:\n          MAILCRAFT_RELEASE_KEY: ${{ secrets.MAILCRAFT_RELEASE_KEY }}\n          MAILCRAFT_XTASK: ${{ github.workspace }}/target/release/xtask\n        run: packaging/sign-sums.sh artifacts",
    );
    let p = problems(&text);
    assert!(p.iter().any(|p| p.contains("job `sign`") && p.contains("runs `cargo`") && p.contains("MAILCRAFT_RELEASE_KEY")), "{p:#?}");
    assert!(p.iter().any(|p| p.contains("target/")), "{p:#?}");
    // The job without a key may compile.
    assert!(!p.iter().any(|p| p.contains("job `other`")), "{p:#?}");
}

/// The Windows and macOS jobs as they were: built, then signed, in one job of the environment.
#[test]
fn build_then_sign_in_one_job_fails() {
    let windows = job(
        "      - uses: dtolnay/rust-toolchain@683b588c487a69d01f6c6ddf40f036ff682cc2c8 # master\n      - name: Build (no signing secrets here)\n        run: ./packaging/windows/package.ps1 -Arch x64 -BuildOnly\n      - uses: azure/login@a641126d1b8aa4d1fa005f4f92df94a3a4c4c906 # v3.1.0\n      - run: ./packaging/windows/package.ps1 -Arch x64 -SkipBuild",
    );
    let p = problems(&windows);
    assert!(p.iter().any(|p| p.contains("signs in to Azure") && p.contains("dtolnay/rust-toolchain")), "{p:#?}");
    assert!(p.iter().any(|p| p.contains("without -SkipBuild -NoRun")), "{p:#?}");

    let macos = job(
        "      - run: bash packaging/macos/package.sh --arch universal --build-only\n      - run: bash packaging/macos/import-cert.sh\n      - run: bash packaging/macos/package.sh --arch universal --skip-build\n      - run: |\n          \"$app/Contents/MacOS/MailCraft\" --version",
    );
    let p = problems(&macos);
    assert!(p.iter().filter(|p| p.contains("without --skip-build --no-run")).count() == 2, "{p:#?}");
    assert!(p.iter().any(|p| p.contains("MacOS/MailCraft")), "{p:#?}");
}

#[test]
fn the_release_environment_makes_a_job_keyed_in_either_form() {
    let inline = "jobs:\n  a:\n    environment: ${{ inputs.sign && 'release' || '' }}\n    steps:\n      - run: rustup show\n";
    assert!(problems(inline).iter().any(|p| p.contains("`release` environment") && p.contains("rustup")));
    let block = "jobs:\n  a:\n    environment:\n      name: release\n      url: https://example.org\n    steps:\n      - run: rustc --version\n";
    assert!(problems(block).iter().any(|p| p.contains("`release` environment") && p.contains("rustc")));
    let other = "jobs:\n  a:\n    environment: github-pages\n    steps:\n      - run: cargo build\n";
    assert!(problems(other).is_empty());
}

#[test]
fn what_makes_a_job_keyed() {
    for keyed in [
        "      - run: echo \"$X\"\n        env:\n          X: ${{ secrets.APPLE_CERTIFICATE }}",
        "      - run: echo ${{ secrets['AZURE_CLIENT_ID'] }}",
        "      - run: security import cert.p12 -k build.keychain",
        "      - uses: apple-actions/import-codesign-certs@0000000000000000000000000000000000000000",
    ] {
        let text = job(&format!("{keyed}\n      - run: cargo build"));
        assert!(problems(&text).iter().any(|p| p.contains("job `sign`")), "{keyed}");
    }
    // The token every job has, and the Google desktop client, are not signing material.
    for not_keyed in [
        "${{ secrets.GITHUB_TOKEN }}",
        "${{ secrets.MAILCRAFT_GOOGLE_CLIENT_ID }}",
        "${{ secrets.MAILCRAFT_GOOGLE_CLIENT_SECRET }}",
        "${{ mysecrets.X }}",
    ] {
        let text = job(&format!("      - run: echo {not_keyed}\n      - run: cargo build"));
        assert!(problems(&text).is_empty(), "{not_keyed}: {:#?}", problems(&text));
    }
    // A secret in the workflow's own env reaches every job.
    let top = "env:\n  K: ${{ secrets.MAILCRAFT_RELEASE_KEY }}\njobs:\n  a:\n    steps:\n      - run: cargo test\n";
    assert!(problems(top).iter().any(|p| p.contains("job `a`")));
    // Through a reusable workflow: the caller inherits every secret.
    let inherit = "jobs:\n  a:\n    uses: ./.github/workflows/x.yml\n    secrets: inherit\n  b:\n    steps:\n      - run: cargo test\n";
    assert!(problems(inherit).is_empty(), "a caller runs no step of its own");
}

#[test]
fn comments_and_longer_names_are_not_commands() {
    let text = job(
        "      # cargo, rustup and target/release/xtask never run here\n      - name: Sign  # not cargo\n        env:\n          K: ${{ secrets.MAILCRAFT_RELEASE_KEY }}\n          CARGO_TARGET_DIR: ${{ github.workspace }}/prebuilt\n        run: |\n          . ~/.cargo-free/env\n          cargo-deny-like-tool\n          ./packaging/windows/package.ps1 -Arch x64 -SkipBuild -NoRun\n          bash packaging/macos/package.sh --arch universal --skip-build --no-run",
    );
    assert!(problems(&text).is_empty(), "{:#?}", problems(&text));
}

#[test]
fn every_job_is_read_up_to_the_next() {
    let text = "jobs:\n  a:\n    environment: release\n    steps:\n      - run: echo signed\n  b:\n    steps:\n      - run: cargo build\n  c:\n    environment: release\n    steps:\n      - run: |\n          make\n          cargo build\n";
    let p = problems(text);
    assert_eq!(p.len(), 1, "{p:#?}");
    assert!(p.first().is_some_and(|p| p.starts_with("w.yml:14: job `c`")), "{p:#?}");
}

#[test]
fn read_all_and_write_all_fail() {
    assert!(problems("permissions: read-all\njobs: {}\n").iter().any(|p| p.contains("w.yml:1:") && p.contains("read-all")));
    assert!(problems("jobs:\n  a:\n    permissions: 'write-all'\n").iter().any(|p| p.contains("write-all")));
    assert!(problems("permissions: {}\njobs:\n  a:\n    permissions:\n      contents: read\n").is_empty());
}

#[test]
fn curl_must_be_https_only() {
    let bad = [
        "      - run: curl -fsSL -o x https://example.org/x",
        "      - run: |\n          curl -fsSL -o \"$RUNNER_TEMP/x.deb\" \\\n            \"https://example.org/x.deb\"",
        "      - run: sudo curl --proto =http https://example.org",
        "      - run: x=$(curl -s https://example.org)",
        "      - run: echo a && /usr/bin/curl https://example.org",
    ];
    for b in bad {
        assert!(problems(&job(b)).iter().any(|p| p.contains("curl without")), "{b}");
    }
    let good = [
        "      - run: curl --proto =https --proto-redir =https -fsSL -o x https://example.org/x",
        "      - run: curl --proto '=https' --tlsv1.2 -fsSL -o /tmp/x https://example.org/x",
        "      - run: |\n          curl -fsSL \\\n            --proto \"=https\" -o x https://example.org/x",
        "      - run: sudo apt-get install -y curl git",
        "      - run: pkg install -y bash git curl pkgconf",
        "      # curl https://example.org",
    ];
    for g in good {
        let p: Vec<String> = problems(&job(g)).into_iter().filter(|p| p.contains("curl")).collect();
        assert!(p.is_empty(), "{g}: {p:#?}");
    }
}
