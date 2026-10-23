#!/usr/bin/env bash
# Regenerate every app icon: the PNGs, the Windows .ico, the macOS .icns, the Linux hicolor tree
# and the SVG.
#
# The artwork is drawn in code (xtask/src/icon.rs), so this needs nothing but cargo. The outputs
# are committed, so builds and packaging never need to run it. Change the drawing there, run this,
# and commit what changed (every file is listed in ATTRIBUTION.md).
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
cargo run --locked -q -p xtask -- icon
