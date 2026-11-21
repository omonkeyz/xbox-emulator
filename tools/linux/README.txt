Building and testing on Linux
=============================

The runtime is a set of Windows DLLs. These scripts cross-compile it for
x86_64-pc-windows-msvc inside a container and run the results under Wine, with
vkd3d-proton translating Direct3D 12 to Vulkan. Nothing is installed on the host
except the container image.

Needs docker (or podman: TOMBOX_LINUX_ENGINE=podman) and a Vulkan driver.

Set up, once
------------

    docker build -t tombox-linux --build-arg UID=$(id -u) --build-arg GID=$(id -g) tools/linux
    tools/linux/run.sh tools/linux/setup-sdk.sh --accept-license

setup-sdk.sh downloads, into ~/.cache/tombox-linux and never into the repository:

  - the Windows SDK and MSVC runtime headers and import libraries, with xwin.
    These are Microsoft's and need their licence accepted: --accept-license
    does that. Run the script without it to read the terms first.
  - the DirectX Shader Compiler, for Windows (loaded under Wine) and for Linux
    (compiles the runtime's own HLSL at build time).
  - vkd3d-proton, installed into a Wine prefix in the cache.

Use
---

    tools/linux/run.sh cargo build -p xhost -p mfplat -p xaudio2_9 -p xmem -p d3d12_xs \
        -p xcrdapi -p dxcompiler_xs -p acphal -p audiohalwrapper
    tools/linux/run.sh cargo test -p tombox-xshader --lib --test opcodes --test synthetic
    tools/linux/run.sh            (a shell in the container)

Build output goes to ~/.cache/tombox-linux/target, not ./target. `cargo test` and
`cargo run` start the Windows executable under Wine (wine-run.sh), with a virtual
display when there is none.

TOMBOX_*, VK_*, VKD3D_* and MESA_* variables set by the caller are passed into
the container. Tests that execute shaders need a Direct3D 12 device:

    TOMBOX_EXEC_HW=1 VK_LOADER_DRIVERS_SELECT='*intel*' \
        tools/linux/run.sh cargo test -p tombox-xshader --test exec -- --test-threads=1

VK_LOADER_DRIVERS_SELECT picks the Vulkan driver: '*intel*', '*radeon*', or
'*lvp*' for Mesa's software renderer (llvmpipe). TOMBOX_EXEC_HW=1 is needed
because Wine has no WARP adapter for the default path to enumerate.

NVIDIA (untested)
-----------------

Mesa's drivers reach the GPU through /dev/dri, which run.sh passes in. NVIDIA's
proprietary driver does not work that way: the container needs the driver's own
libraries, which the NVIDIA Container Toolkit injects. When the host has
nvidia-smi and the container engine knows the nvidia runtime, run.sh adds
`--gpus all` (podman: `--device nvidia.com/gpu=all`) and asks for all driver
capabilities, so the Vulkan ICD is included. Select the driver with

    VK_LOADER_DRIVERS_SELECT='*nvidia*'

This path was written from the toolkit's documentation and has not been run on
an NVIDIA machine. TOMBOX_LINUX_GPU=dri, nvidia or none overrides the detection.

How it differs from a Windows build
-----------------------------------

  - Linker: lld-link, not link.exe. Librarian: tools/linux/msvc-lib.py stands in
    for lib.exe. Build scripts that turn a .def into an .exp get an object file
    of /EXPORT directives instead, which lld-link reads the same way.
  - query_accum.hlsl is compiled by the Linux dxc (TOMBOX_DXC).

Known limits
------------

Measured on an Intel i7-1165G7 (Iris Xe), Mesa 26.2, Wine 11.21, vkd3d-proton 3.1.

  - The eight default runtime DLLs build and load under Wine, and the host
    harness (xhost --no-epic) runs to its summary: 10 pass, 2 fail, 2 skip. Both
    failures are the built-in online backend, which needs the private
    backend_data at build time. The skips need Gaming Services. No title has
    been run this way.
  - Of the functions those DLLs import, Wine 11.21 lacks three:
    ntdll!NtOpenPartition, ntdll!NtManagePartition and
    iphlpapi!GetNetworkConnectivityHint. Wine loads a missing import as a stub
    that aborts the process when called, so the runtime looks these up at run
    time and takes its existing fallback when they are absent.
  - vkd3d-proton refuses shaders that use 64-bit floats on Intel (no
    shaderFloat64) and on llvmpipe. The translator's exact v_fma_f32 path and
    the f64 instructions use them, so those tests fail at pipeline creation.
  - llvmpipe reports below feature level 12_0 under vkd3d-proton. The test
    harness falls back to the level the device has.
  - Intel Xe-LP has no mesh shaders. vkd3d-proton disables them on llvmpipe.
