#!/usr/bin/env bash
# Cargo runner for the Windows target: run a test or tool executable under Wine.
# A virtual display is started when there is none, so windowed tests run headless.
set -euo pipefail
if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ]; then
    exec xvfb-run -a wine "$@"
fi
exec wine "$@"
