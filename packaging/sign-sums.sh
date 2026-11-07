#!/usr/bin/env bash
# Sign a release's SHA256SUMS.txt with MailCraft's own release key: writes SHA256SUMS.txt.sig
# beside it (an ed25519 signature in base64, docs/releasing.md). A copy of MailCraft installs an
# update only when this signature verifies by one of the public keys it carries.
#
#   MAILCRAFT_RELEASE_KEY=… packaging/sign-sums.sh <folder with SHA256SUMS.txt>
#   packaging/sign-sums.sh --self-test
#
# The key (base64 PKCS#8, from `cargo xtask release-key new`) is a secret of the protected
# `release` environment and comes in through the environment only; it is never printed or
# written to a file. Without it the release is unsigned, said as such, and the build carries on
# (forks, dry runs).
#
# The signature is made by OpenSSL 3 (`openssl pkeyutl -sign -rawin`: plain ed25519 over the
# file's bytes, the same bytes `cargo xtask release-key sign` gives), not by anything compiled
# from this repository: the job that holds the key never runs cargo or a program it built. In
# GitHub Actions the OpenSSL is the runner's own package, checked against the package's file list
# (`dpkg --verify`) before the key is handed to it. Locally, without OpenSSL 3, it falls back to
# `cargo run --package xtask -- release-key sign`.
#
# --self-test signs and verifies a scratch file with a throwaway key, so a dry run proves the
# signing path without the release key.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

openssl3() {
  command -v openssl >/dev/null 2>&1 && openssl version 2>/dev/null | grep -Eq '^OpenSSL [3-9]\.'
}

# In Actions: the runner's OpenSSL, as its package shipped it, or nothing.
checked_openssl() {
  if ! openssl3; then
    echo "::error::OpenSSL 3 isn't on this runner: the checksums can't be signed"
    exit 1
  fi
  if [ -n "${GITHUB_ACTIONS:-}" ] && command -v dpkg >/dev/null 2>&1; then
    local bin lib file pkg
    bin="$(readlink -f "$(command -v openssl)")"
    lib="$(ldd "$bin" | awk '/libcrypto/ { print $3; exit }')"
    for file in "$bin" ${lib:+"$(readlink -f "$lib")"}; do
      pkg="$(dpkg -S "$file" 2>/dev/null | cut -d: -f1 | head -n 1)"
      if [ -z "$pkg" ] || [ -n "$(dpkg --verify "$pkg" 2>&1)" ]; then
        echo "::error::$file isn't as its package (${pkg:-none}) installed it"
        exit 1
      fi
      # shellcheck disable=SC2016 # dpkg-query's own format, not the shell's
      echo "$file: $pkg $(dpkg-query -W -f='${Version}' "$pkg"), as installed"
    done
    echo "signing with $(openssl version)"
  fi
}

# The key's DER bytes on stdout, from the environment variable named by $1.
key_der() {
  printf '%s' "${!1}" | tr -d '[:space:]' | base64 -d
}

# sign <file> <variable holding the key>: <file>.sig, and the public key said.
sign_with_openssl() {
  local file="$1" var="$2" public spki
  # An ed25519 key's public half is 44 bytes of DER: this prefix (id-Ed25519) and 32 bytes.
  spki="$(key_der "$var" 2>/dev/null | openssl pkey -inform DER -pubout -outform DER 2>/dev/null | od -An -v -tx1 | tr -d ' \n' || true)"
  if [ "${#spki}" != 88 ] || [ "${spki#302a300506032b6570032100}" = "$spki" ]; then
    echo "::error::$var isn't an ed25519 private key in base64 PKCS#8 (as \`cargo xtask release-key new\` writes it)"
    exit 1
  fi
  openssl pkeyutl -sign -rawin -keyform DER -inkey <(key_der "$var") -in "$file" | base64 | tr -d '\n' >"$file.sig.part"
  echo >>"$file.sig.part"
  mv "$file.sig.part" "$file.sig"
  public="$(key_der "$var" | openssl pkey -inform DER -pubout -outform DER | tail -c 32 | base64)"
  echo "signed $file with the release key $public: $file.sig"
}

if [ "${1:-}" = "--self-test" ]; then
  checked_openssl
  dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/sign-sums.XXXXXX")"
  trap 'rm -rf "$dir"' EXIT
  printf '0123  mailcraft-0.0.0-self-test\n' >"$dir/SHA256SUMS.txt"
  # shellcheck disable=SC2034 # read by name in key_der
  SELF_TEST_KEY="$(openssl genpkey -algorithm ed25519 -outform DER | base64 | tr -d '\n')"
  sign_with_openssl "$dir/SHA256SUMS.txt" SELF_TEST_KEY
  key_der SELF_TEST_KEY | openssl pkey -inform DER -pubout -outform DER >"$dir/public.der"
  base64 -d <"$dir/SHA256SUMS.txt.sig" >"$dir/sig.bin"
  [ "$(wc -c <"$dir/sig.bin" | tr -d ' ')" = 64 ] || { echo "::error::the self-test signature isn't 64 bytes"; exit 1; }
  openssl pkeyutl -verify -rawin -pubin -keyform DER -inkey "$dir/public.der" -in "$dir/SHA256SUMS.txt" -sigfile "$dir/sig.bin"
  echo "self-test: the signing path signs and verifies"
  exit 0
fi

DIR="${1:?usage: packaging/sign-sums.sh <folder with SHA256SUMS.txt> | --self-test}"
SUMS="$(cd "$DIR" && pwd)/SHA256SUMS.txt"

if [ ! -f "$SUMS" ]; then
  echo "::error::$SUMS isn't there"
  exit 1
fi
rm -f "$SUMS.sig"
if [ -z "${MAILCRAFT_RELEASE_KEY:-}" ]; then
  echo "unsigned: MAILCRAFT_RELEASE_KEY isn't set, so there is no SHA256SUMS.txt.sig and no copy of MailCraft will install this release by itself"
  exit 0
fi
if [ -n "${GITHUB_ACTIONS:-}" ] || openssl3; then
  checked_openssl
  sign_with_openssl "$SUMS" MAILCRAFT_RELEASE_KEY
  exit 0
fi
cd "$ROOT"
cargo run --locked --quiet --package xtask -- release-key sign "$SUMS"
