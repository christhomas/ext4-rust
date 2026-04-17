#!/bin/bash
# Wrapper for build-ext4-feature-images.sh so the filename matches CI's
# invocation convention. Regenerates the entire matrix of ext4 fixture
# images in-place. Requires docker (macOS lacks mkfs.ext4).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
exec bash "$SCRIPT_DIR/build-ext4-feature-images.sh" "$@"
