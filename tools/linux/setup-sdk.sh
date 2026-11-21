#!/usr/bin/env bash
# Fetch what the Linux build needs but the image does not contain. Run it inside the
# container:  tools/linux/run.sh tools/linux/setup-sdk.sh --accept-license
#
# 1. The Windows SDK and MSVC C runtime headers and import libraries, downloaded from
#    Microsoft by xwin (https://github.com/Jake-Shadle/xwin). They are covered by the
#    Microsoft Software License Terms: https://go.microsoft.com/fwlink/?LinkId=2086102
#    Pass --accept-license only if you accept those terms.
# 2. The DirectX Shader Compiler: the Windows DLLs the runtime loads under Wine and the
#    Linux build that compiles the runtime's own HLSL at build time.
#    https://github.com/microsoft/DirectXShaderCompiler (see its LICENSE.TXT).
# 3. vkd3d-proton, which implements Direct3D 12 on Vulkan under Wine (LGPL-2.1).
#    https://github.com/HansKristian-Work/vkd3d-proton
#
# Nothing here is copied into the repository.
set -euo pipefail

XWIN_VERSION=0.10.0
DXC_TAG=v1.9.2609
DXC_WIN=dxc_2026_09_29.zip
DXC_LINUX=linux_dxc_2026_09_28.x86_x64.tar.gz
VKD3D_VERSION=3.1

if [ "${1:-}" != "--accept-license" ]; then
    sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
    echo
    echo "Re-run with --accept-license to accept the Microsoft licence and download."
    exit 1
fi

cache=/cache
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if [ ! -d "$cache/xwin/crt/lib/x86_64" ]; then
    echo "== Windows SDK and CRT (xwin $XWIN_VERSION)"
    curl -fsSL "https://github.com/Jake-Shadle/xwin/releases/download/$XWIN_VERSION/xwin-$XWIN_VERSION-x86_64-unknown-linux-musl.tar.gz" \
        | tar -xz -C "$tmp"
    "$tmp/xwin-$XWIN_VERSION-x86_64-unknown-linux-musl/xwin" --accept-license --arch x86_64 \
        --cache-dir "$cache/xwin-download" splat --output "$cache/xwin"
    rm -rf "$cache/xwin-download"
fi

if [ ! -f "$cache/dxc/bin/x64/dxcompiler.dll" ] || [ ! -x "$cache/dxc/linux/bin/dxc" ]; then
    echo "== DirectX Shader Compiler $DXC_TAG"
    base="https://github.com/microsoft/DirectXShaderCompiler/releases/download/$DXC_TAG"
    curl -fsSL "$base/$DXC_WIN" -o "$tmp/dxc.zip"
    # The archive uses backslash separators; unzip converts them and exits 1 with a warning.
    unzip -q -o "$tmp/dxc.zip" -d "$cache/dxc" || [ $? -eq 1 ]
    mkdir -p "$cache/dxc/linux"
    curl -fsSL "$base/$DXC_LINUX" | tar -xz -C "$cache/dxc/linux"
fi

if [ ! -f "$cache/vkd3d-proton/x64/d3d12.dll" ]; then
    echo "== vkd3d-proton $VKD3D_VERSION"
    curl -fsSL "https://github.com/HansKristian-Work/vkd3d-proton/releases/download/v$VKD3D_VERSION/vkd3d-proton-$VKD3D_VERSION.tar.zst" \
        | tar --zstd -x -C "$tmp"
    mkdir -p "$cache/vkd3d-proton"
    cp -r "$tmp/vkd3d-proton-$VKD3D_VERSION/x64" "$cache/vkd3d-proton/"
fi

if [ ! -f "$cache/wine/.vkd3d-proton-$VKD3D_VERSION" ]; then
    echo "== Wine prefix"
    wineboot --init >/dev/null 2>&1
    wineserver -w
    sys32="$WINEPREFIX/drive_c/windows/system32"
    for dll in d3d12 d3d12core; do
        cp "$cache/vkd3d-proton/x64/$dll.dll" "$sys32/$dll.dll"
        wine reg add 'HKCU\Software\Wine\DllOverrides' /v "$dll" /d native /f >/dev/null 2>&1
    done
    wineserver -w
    touch "$cache/wine/.vkd3d-proton-$VKD3D_VERSION"
fi

echo "ready: $(du -sh "$cache/xwin" | cut -f1) SDK, DXC $DXC_TAG, vkd3d-proton $VKD3D_VERSION"
