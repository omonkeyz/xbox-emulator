#!/usr/bin/env bash
# Run a command in the Linux build container, with the repository at /work.
#
#   tools/linux/run.sh cargo build -p xmem
#   tools/linux/run.sh cargo test -p tombox-xshader --test exec
#   tools/linux/run.sh            (an interactive shell)
#
# Everything that should survive between runs (the Windows SDK, the cargo registry, build
# output, the Wine prefix) lives in $TOMBOX_LINUX_CACHE, default ~/.cache/tombox-linux.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cache="${TOMBOX_LINUX_CACHE:-$HOME/.cache/tombox-linux}"
image="${TOMBOX_LINUX_IMAGE:-tombox-linux}"
engine="${TOMBOX_LINUX_ENGINE:-docker}"
mkdir -p "$cache"/{xwin,cargo-registry,cargo-git,target,wine,dxc,vkd3d-shader-cache}

tty=()
[ -t 0 ] && [ -t 1 ] && tty=(-it)
# GPU access. Mesa's drivers (Intel, AMD, llvmpipe) only need /dev/dri. NVIDIA's
# proprietary driver needs the NVIDIA Container Toolkit on the host, which injects its
# libraries and Vulkan ICD. The nvidia branch is UNTESTED: it follows the toolkit's
# documentation and has not been run. TOMBOX_LINUX_GPU=dri|nvidia|none overrides the guess.
gpu=()
gpu_mode="${TOMBOX_LINUX_GPU:-auto}"
if [ "$gpu_mode" = auto ]; then
    gpu_mode=dri
    if command -v nvidia-smi >/dev/null 2>&1 && "$engine" info 2>/dev/null | grep -qi nvidia; then
        gpu_mode=nvidia
    fi
fi
case "$gpu_mode" in
    nvidia)
        if [ "$engine" = podman ]; then gpu=(--device nvidia.com/gpu=all); else gpu=(--gpus all); fi
        gpu+=(-e NVIDIA_VISIBLE_DEVICES=all -e NVIDIA_DRIVER_CAPABILITIES=all)
        [ -e /dev/dri ] && gpu+=(--device /dev/dri)
        ;;
    dri) [ -e /dev/dri ] && gpu=(--device /dev/dri) ;;
    none) ;;
    *) echo "TOMBOX_LINUX_GPU must be auto, dri, nvidia or none (got '$gpu_mode')" >&2; exit 2 ;;
esac

# Pass through the caller's settings for the runtime, Vulkan and Wine, for example
#   TOMBOX_EXEC_HW=1 VK_LOADER_DRIVERS_SELECT='*lvp*' tools/linux/run.sh cargo test ...
pass=()
for name in $(compgen -e); do
    case "$name" in
        TOMBOX_LINUX_*) ;;
        TOMBOX_*|VK_*|VKD3D_*|MESA_*|LP_*|RUST_BACKTRACE|CARGO_TERM_*) pass+=(-e "$name") ;;
    esac
done

xwin=/cache/xwin
exec "$engine" run --rm "${tty[@]}" "${gpu[@]}" "${pass[@]}" \
    -v "$repo":/work -v "$cache":/cache \
    -v "$cache/cargo-registry":/home/dev/.cargo/registry \
    -v "$cache/cargo-git":/home/dev/.cargo/git \
    -e CARGO_BUILD_TARGET=x86_64-pc-windows-msvc \
    -e CARGO_TARGET_DIR=/cache/target \
    -e CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=lld-link \
    -e CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=/work/tools/linux/wine-run.sh \
    -e CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-Lnative=$xwin/crt/lib/x86_64 -Lnative=$xwin/sdk/lib/um/x86_64 -Lnative=$xwin/sdk/lib/ucrt/x86_64" \
    -e CC_x86_64_pc_windows_msvc=clang-cl \
    -e CXX_x86_64_pc_windows_msvc=clang-cl \
    -e AR_x86_64_pc_windows_msvc=llvm-lib \
    -e CFLAGS_x86_64_pc_windows_msvc="/imsvc $xwin/crt/include /imsvc $xwin/sdk/include/ucrt /imsvc $xwin/sdk/include/um /imsvc $xwin/sdk/include/shared" \
    -e CXXFLAGS_x86_64_pc_windows_msvc="/imsvc $xwin/crt/include /imsvc $xwin/sdk/include/ucrt /imsvc $xwin/sdk/include/um /imsvc $xwin/sdk/include/shared" \
    -e WINEPREFIX=/cache/wine \
    -e WINEDEBUG="${WINEDEBUG:--all}" \
    -e VKD3D_SHADER_CACHE_PATH=/cache/vkd3d-shader-cache \
    -e TOMBOX_DXC_DIR=/cache/dxc/bin/x64 \
    -e TOMBOX_DXC=/cache/dxc/linux/bin/dxc \
    "$image" "${@:-bash}"
