#!/usr/bin/env bash
# Sign a release's SHA256SUMS.txt with MailCraft's own release key: writes SHA256SUMS.txt.sig
# beside it (an ed25519 signature in base64, docs/releasing.md). A copy of MailCraft installs an
# update only when this signature verifies by one of the public keys it carries.
#
#   MAILCRAFT_RELEASE_KEY=… packaging/sign-sums.sh <folder with SHA256SUMS.txt>
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
# `cargo run --package xtask -- release-key sign`. The release workflow's dry run checks that both
# make the same signature, with a key `cargo xtask release-key new` made.
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
  if [[ -n "${GITHUB_ACTIONS:-}" ]] && command -v dpkg >/dev/null 2>&1; then
    local bin lib file pkg verify
    bin="$(readlink -f "$(command -v openssl)")"
    lib="$(ldd "$bin" | awk '/libcrypto/ { print $3; exit }')"
    for file in "$bin" ${lib:+"$(readlink -f "$lib")"}; do
      pkg="$(dpkg -S "$file" 2>/dev/null | cut -d: -f1 | head -n 1)"
      # Only this file's line counts: a changed file elsewhere in the package (its config) is
      # not what signs. `dpkg --verify` ends each line with the file's path.
      # (dpkg --verify fails for any changed file, so its status alone says nothing here.)
      verify="$( [[ -z "$pkg" ]] || dpkg --verify "$pkg" 2>&1 || true)"
      if [[ -z "$pkg" ]] || printf '%s\n' "$verify" | awk -v f="$file" '$NF == f { bad = 1 } END { exit !bad }'; then
        echo "::error::$file isn't as its package (${pkg:-none}) installed it"
        exit 1
      fi
      # shellcheck disable=SC2016 # dpkg-query's own format, not the shell's
      echo "$file: $pkg $(dpkg-query -W -f='${Version}' "$pkg"), as installed"
    done
    echo "signing with $(openssl version)"
  fi
}

# Hex on stdin, as bytes on stdout (printf is the shell's own: nothing goes on a command line).
unhex() {
  local escaped
  escaped="$(sed -e 's/../\\x&/g')"
  # shellcheck disable=SC2059 # the format is \xHH escapes of checked hex and nothing else
  printf "$escaped"
}

# The key's DER in hex, from the environment variable named by $1.
key_hex() {
  printf '%s' "${!1}" | tr -d '[:space:]' | base64 -d 2>/dev/null | od -An -v -tx1 | tr -d ' \n'
}

# The PKCS#8 v1 DER of the key whose seed is in KEY_SEED. OpenSSL 3.0 doesn't read the v2 form
# `cargo xtask release-key new` writes (ring's, with the public half in it), so the key goes to
# OpenSSL as v1, rebuilt from its seed; it only ever passes through a pipe.
v1_der() {
  printf '%s' "302e020100300506032b657004220420$KEY_SEED" | unhex
}

# sign <file> <variable holding the key>: <file>.sig, and the public key said.
sign_with_openssl() {
  local file="$1" var="$2" hex embedded="" public spki bad
  bad="$var isn't an ed25519 private key in base64 PKCS#8 (as \`cargo xtask release-key new\` writes it)"
  hex="$(key_hex "$var" || true)"
  KEY_SEED=""
  # Two layouts and no other: v2 as ring writes it (83 bytes: version 1, the seed, then the public
  # half as [1] 33 bytes), and v1 (48 bytes: version 0 and the seed).
  if [[ ${#hex} == 166 ]] && [[ ${hex:0:32} == 3051020101300506032b657004220420 ]] && [[ ${hex:96:6} == 812100 ]]; then
    KEY_SEED="${hex:32:64}"
    embedded="${hex:102:64}"
  elif [[ ${#hex} == 96 ]] && [[ ${hex:0:32} == 302e020100300506032b657004220420 ]]; then
    KEY_SEED="${hex:32:64}"
  fi
  hex=""
  case "$KEY_SEED" in
    "" | *[!0-9a-f]*)
      KEY_SEED=""
      echo "::error::$bad"
      exit 1
      ;;
  esac
  # An ed25519 key's public half is 44 bytes of DER: this prefix (id-Ed25519) and 32 bytes; and in
  # a v2 key it must be the half the key carries.
  spki="$(v1_der | openssl pkey -inform DER -pubout -outform DER 2>/dev/null | od -An -v -tx1 | tr -d ' \n' || true)"
  if [[ ${#spki} != 88 ]] || [[ ${spki:0:24} != 302a300506032b6570032100 ]]; then
    KEY_SEED=""
    echo "::error::$bad"
    exit 1
  fi
  if [[ -n "$embedded" ]] && [[ ${spki:24:64} != "$embedded" ]]; then
    KEY_SEED=""
    echo "::error::$var carries a public key that isn't its own"
    exit 1
  fi
  openssl pkeyutl -sign -rawin -keyform DER -inkey <(v1_der) -in "$file" | base64 | tr -d '\n' >"$file.sig.part"
  KEY_SEED=""
  echo >>"$file.sig.part"
  mv "$file.sig.part" "$file.sig"
  public="$(printf '%s' "${spki:24:64}" | unhex | base64)"
  echo "signed $file with the release key $public: $file.sig"
}

DIR="${1:?usage: packaging/sign-sums.sh <folder with SHA256SUMS.txt>}"
SUMS="$(cd "$DIR" && pwd)/SHA256SUMS.txt"

if [[ ! -f "$SUMS" ]]; then
  echo "::error::$SUMS isn't there"
  exit 1
fi
rm -f "$SUMS.sig"
if [[ -z "${MAILCRAFT_RELEASE_KEY:-}" ]]; then
  echo "unsigned: MAILCRAFT_RELEASE_KEY isn't set, so there is no SHA256SUMS.txt.sig and no copy of MailCraft will install this release by itself"
  exit 0
fi
if [[ -n "${GITHUB_ACTIONS:-}" ]] || openssl3; then
  checked_openssl
  sign_with_openssl "$SUMS" MAILCRAFT_RELEASE_KEY
  exit 0
fi
cd "$ROOT"
cargo run --locked --quiet --package xtask -- release-key sign "$SUMS"
