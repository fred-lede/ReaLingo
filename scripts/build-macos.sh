#!/usr/bin/env bash
set -euo pipefail
# macOS keeps the system headers inside the SDK, and bindgen invokes libclang directly
# (no xcrun driver), so llama-cpp-sys-2's bindings step dies on "'stdio.h' file not
# found" unless the SDK is in the environment.
export SDKROOT="$(xcrun --show-sdk-path)"

# A killed DMG step leaves its staging image inside bundle/macos/, and the next
# bundle_dmg.sh run then fails with "no space left on device" while copying it in.
rm -f src-tauri/target/release/bundle/macos/rw.*.dmg

npm run tauri build "$@"
