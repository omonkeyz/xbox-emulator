# Linux build and test environment for the runtime.
#
# The runtime is a set of Windows DLLs for the x86_64-pc-windows-msvc target. This image
# cross-compiles them with LLVM's MSVC-compatible tools (clang-cl, lld-link, llvm-lib) and
# runs the results under Wine, with vkd3d-proton translating Direct3D 12 to Vulkan.
#
# It holds no Microsoft files. The Windows SDK and C runtime libraries the linker needs are
# fetched separately by tools/linux/setup-sdk.sh, which asks you to accept their licence.
#
#   docker build -t tombox-linux --build-arg UID=$(id -u) --build-arg GID=$(id -g) tools/linux
FROM registry.fedoraproject.org/fedora:44

RUN dnf install -y --setopt=install_weak_deps=False \
        wine-core wine-common \
        mesa-vulkan-drivers mesa-dri-drivers vulkan-loader vulkan-tools \
        xorg-x11-server-Xvfb \
        clang lld llvm gcc \
        git curl zstd xz unzip tar python3 which file findutils procps-ng \
    && dnf clean all

ARG UID=1000
ARG GID=1000
RUN groupadd -g ${GID} dev && useradd -m -u ${UID} -g ${GID} dev
USER dev
ENV HOME=/home/dev
WORKDIR /home/dev

ARG RUST_VERSION=1.100.0
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal --default-toolchain ${RUST_VERSION} \
    && $HOME/.cargo/bin/rustup target add x86_64-pc-windows-msvc
ENV PATH=/home/dev/.cargo/bin:/home/dev/bin:$PATH

# The build scripts look for the MSVC librarian by the name `lib.exe` on PATH.
# tools/linux/msvc-lib.py stands in for it (see that file).
RUN mkdir -p $HOME/bin \
    && printf '#!/bin/sh\nexec /work/tools/linux/msvc-lib.py "$@"\n' > $HOME/bin/lib.exe \
    && chmod +x $HOME/bin/lib.exe

WORKDIR /work
