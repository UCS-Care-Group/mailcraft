#!/usr/bin/env bash
# Hand files from one release job to another by digest (docs/releasing.md).
#
#   packaging/ci/handoff.sh seal <dir> <name>
#       Writes <dir>/<name>.sha256 (the SHA-256 of every file under <dir>, `sha256sum` format)
#       and prints the SHA-256 of that list: the digest the job gives as an output.
#   packaging/ci/handoff.sh check <dir> <name>=<digest> [<name>?=<digest>]...
#       In a later job, after the artifacts are downloaded into <dir>: each list must have the
#       digest the job that made it gave, every file in it must match, and nothing may be in <dir>
#       that no list names. The lists are then removed. A name ending in `?` may be missing
#       altogether (no list and no digest: a build allowed to fail, FreeBSD's).
#
# The digest travels as a job output, not with the artifact, so a file changed or added between
# the two jobs stops the run. Bash 3.2 (macOS) and Git Bash (Windows) run it too.
set -euo pipefail

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

die() {
  echo "::error::$*" >&2
  exit 1
}

# Every regular file under the current folder, `./`-relative, in a fixed order.
files() {
  find . -type f | LC_ALL=C sort
}

seal() {
  local dir="$1" name="$2" list
  [[ -d "$dir" ]] || die "$dir isn't a folder"
  case "$name" in *[!A-Za-z0-9._-]* | "") die "bad handoff name: $name" ;; esac
  list="$name.sha256"
  (
    cd "$dir"
    rm -f -- "$list"
    files >"$RUNNER_TEMP_LIST"
    [[ -s "$RUNNER_TEMP_LIST" ]] || die "$dir has no files to hand on"
    while IFS= read -r f; do sha256 "$f"; done <"$RUNNER_TEMP_LIST" >"$list.part"
    mv -- "$list.part" "$list"
    sha256 "$list" | cut -d' ' -f1
  )
}

check() {
  local dir="$1" pair name digest optional list got
  shift
  [[ -d "$dir" ]] || die "$dir isn't a folder"
  cd "$dir"
  : >"$RUNNER_TEMP_LIST.named"
  for pair in "$@"; do
    name="${pair%%=*}"
    digest="${pair#*=}"
    optional=0
    case "$name" in *\?) optional=1; name="${name%\?}" ;; esac
    case "$name" in *[!A-Za-z0-9._-]* | "") die "bad handoff name: $name" ;; esac
    list="$name.sha256"
    if [[ -z "$digest" ]] && [[ ! -e "$list" ]] && [[ $optional == 1 ]]; then
      echo "$name: not built, nothing to check"
      continue
    fi
    case "$digest" in
      "") die "$name: the job that made it gave no digest" ;;
      *[!0-9a-f]*) die "$name: the digest isn't SHA-256 in hex" ;;
    esac
    [[ ${#digest} == 64 ]] || die "$name: the digest isn't SHA-256 in hex"
    [[ -f "$list" ]] || die "$name: $list isn't among the downloaded files"
    got="$(sha256 "$list" | cut -d' ' -f1)"
    [[ $got == "$digest" ]] || die "$name: its list of files has SHA-256 $got, but the job that made it gave $digest"
    # Every name is under this folder: `./`, and no `..` on the way.
    while IFS= read -r f; do
      case "$f" in
        ./*) ;;
        *) die "$name: $list names \"$f\", which isn't under ./" ;;
      esac
      case "$f" in
        */../* | */..) die "$name: $list names \"$f\", which leaves the folder" ;;
      esac
    done < <(sed -e 's/^[0-9a-f]\{64\} [ *]//' "$list")
    sha256 -c "$list" || die "$name: a file doesn't match the SHA-256 its job gave it"
    echo "./$list" >>"$RUNNER_TEMP_LIST.named"
    # The names in the list, as `files` prints them (`sha256sum` puts a `*` before a binary one).
    sed -e 's/^[0-9a-f]\{64\} [ *]//' "$list" >>"$RUNNER_TEMP_LIST.named"
  done
  files >"$RUNNER_TEMP_LIST"
  LC_ALL=C sort -o "$RUNNER_TEMP_LIST.named" "$RUNNER_TEMP_LIST.named"
  if ! LC_ALL=C comm -23 "$RUNNER_TEMP_LIST" "$RUNNER_TEMP_LIST.named" >"$RUNNER_TEMP_LIST.extra"; then
    die "could not compare the downloaded files with their lists"
  fi
  if [[ -s "$RUNNER_TEMP_LIST.extra" ]]; then
    cat "$RUNNER_TEMP_LIST.extra" >&2
    die "files no job's list names were downloaded"
  fi
  for pair in "$@"; do
    name="${pair%%=*}"
    rm -f -- "${name%\?}.sha256"
  done
  echo "every file matches the digest of the job that made it"
}

TMP="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
# Git Bash on Windows: RUNNER_TEMP is a Windows path.
if command -v cygpath >/dev/null 2>&1; then TMP="$(cygpath -u "$TMP")"; fi
RUNNER_TEMP_LIST="$(mktemp "$TMP/handoff.XXXXXX")"
trap 'rm -f "$RUNNER_TEMP_LIST" "$RUNNER_TEMP_LIST".*' EXIT

case "${1:-}" in
  seal) [[ $# == 3 ]] || die "usage: handoff.sh seal <dir> <name>"; seal "$2" "$3" ;;
  check) [[ $# -ge 3 ]] || die "usage: handoff.sh check <dir> <name>=<digest>..."; shift; check "$@" ;;
  *) die "usage: handoff.sh seal <dir> <name> | check <dir> <name>=<digest>..." ;;
esac
