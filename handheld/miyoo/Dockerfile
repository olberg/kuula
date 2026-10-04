# syntax=docker/dockerfile:1
#
# The Miyoo Mini (Plus) build: an armv7 glibc `kuula` without networking,
# linked against the steward-fu SDL2 fork that OnionOS ports ship. Build
# it from the repository root:
#
#   docker build -f handheld/miyoo/Dockerfile --target artifacts \
#       --output type=local,dest=dist/miyoo-build .
#
# The device's glibc is 2.28 and its userland is GCC 8 era. Debian 10
# (buster) is the same pair: gcc 8.3.0 and glibc 2.28, both as an armhf
# cross toolchain, so the binary asks for no symbol the device lacks. The
# union-miyoomini-toolchain the research notes name is that pair built
# from Arm's sources; nothing here needs its blobs.
#
# Buster is out of support: its packages come from archive.debian.org.

ARG RUST_VERSION=1.98.1

# ---- the SDL2 fork and the SigmaStar libraries, pinned and checked ----------
FROM debian:buster-slim AS sdl
RUN sed -i 's|deb.debian.org|archive.debian.org|g; /security.debian.org/d; /buster-updates/d' /etc/apt/sources.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# steward-fu/sdl2 commit 0631abc8 (2026-04-19). The 640x480 prebuilt is the
# one whose Mini driver source is the tree at that commit (DEF_FB_W 640).
# The libmi_* files are SigmaStar's own, taken from the same repository for
# linking only; the device has its own copies in /config/lib.
ARG STEWARD=https://raw.githubusercontent.com/steward-fu/sdl2/0631abc8e8916db6f9bc7e2afd0c22913d092a29
WORKDIR /opt/miyoo/lib
RUN set -e; \
    for f in libSDL2-2.0.so.0 libEGL.so libGLESv2.so; do \
        curl -fsSL -o $f $STEWARD/prebuilt/640x480/$f; done; \
    for f in libjson-c.so.5 libz.so.1; do \
        curl -fsSL -o $f $STEWARD/examples/$f; done; \
    for f in libmi_ao libmi_common libmi_gfx libmi_sys libshmvar libssgfx; do \
        curl -fsSL -o $f.so $STEWARD/mini/lib/$f.so; done
COPY handheld/miyoo/sdl2.sha256 /tmp/sdl2.sha256
RUN sha256sum -c /tmp/sdl2.sha256

# ---- the cross toolchain ------------------------------------------------------
FROM debian:buster-slim AS toolchain
RUN sed -i 's|deb.debian.org|archive.debian.org|g; /security.debian.org/d; /buster-updates/d' /etc/apt/sources.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl gcc libc6-dev make pkg-config \
        gcc-arm-linux-gnueabihf libc6-dev-armhf-cross binutils-arm-linux-gnueabihf \
    && rm -rf /var/lib/apt/lists/*
ARG RUST_VERSION
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal \
        --default-toolchain ${RUST_VERSION} --target armv7-unknown-linux-gnueabihf

COPY --from=sdl /opt/miyoo /opt/miyoo
# `sdl2-sys` finds SDL through pkg-config off Windows. The fork's headers
# are not needed (the crate ships its own bindings), only the library name.
RUN ln -s libSDL2-2.0.so.0 /opt/miyoo/lib/libSDL2.so \
    && mkdir -p /opt/miyoo/lib/pkgconfig \
    && printf '%s\n' \
        'prefix=/opt/miyoo' \
        'libdir=${prefix}/lib' \
        'Name: sdl2' \
        'Description: steward-fu SDL2 fork for the Miyoo Mini (Plus)' \
        'Version: 2.0.18' \
        'Libs: -L${libdir} -lSDL2' \
        > /opt/miyoo/lib/pkgconfig/sdl2.pc
# libSDL2 needs libEGL, libGLESv2, libjson-c and the libmi_* set by name;
# the linker checks them, so they sit on its search path (-rpath-link does
# not put them in the binary). The compiler flags for the C (Lua) build
# live in .cargo/config.toml beside the seed pin.
ENV PKG_CONFIG_ALLOW_CROSS=1 \
    PKG_CONFIG_LIBDIR=/opt/miyoo/lib/pkgconfig \
    CC_armv7_unknown_linux_gnueabihf=arm-linux-gnueabihf-gcc \
    CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUSTFLAGS="-C link-arg=-Wl,-rpath-link,/opt/miyoo/lib"
WORKDIR /src

# ---- the build ---------------------------------------------------------------
FROM toolchain AS build
COPY . .
ENV CARGO_TARGET_DIR=/cache/target
# Release binary: no `net` feature, so no tokio, QUIC or TLS.
RUN --mount=type=cache,id=kuula-miyoo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=kuula-miyoo-target,target=/cache/target \
    set -e; \
    cargo build --release --locked --target armv7-unknown-linux-gnueabihf \
        -p kuula-cli --no-default-features; \
    mkdir -p /out/tests; \
    cp /cache/target/armv7-unknown-linux-gnueabihf/release/kuula /out/kuula; \
    arm-linux-gnueabihf-strip -o /out/kuula.stripped /out/kuula; \
    arm-linux-gnueabihf-readelf -d /out/kuula.stripped | grep -E 'NEEDED|RPATH|RUNPATH' > /out/needed.txt; \
    arm-linux-gnueabihf-objdump -T /out/kuula.stripped | grep -o 'GLIBC_[0-9.]*' | sort -uV > /out/glibc-symbols.txt; \
    cp /opt/miyoo/lib/libSDL2-2.0.so.0 /opt/miyoo/lib/libEGL.so /opt/miyoo/lib/libGLESv2.so \
       /opt/miyoo/lib/libjson-c.so.5 /opt/miyoo/lib/libz.so.1 /out/; \
    cp /usr/arm-linux-gnueabihf/lib/libgcc_s.so.1 /out/;     mkdir -p /out/link-libs;     cp /opt/miyoo/lib/libmi_*.so /opt/miyoo/lib/libshmvar.so /opt/miyoo/lib/libssgfx.so /out/link-libs/
# link-libs holds the SigmaStar libraries libSDL2 names; the device has its
# own, and they are here only so the binary can be loaded under emulation.
# The conformance oracles as armv7 test binaries, to run under emulation
# (they need no SDL, only the examples directory).
# `omt` pins the song engine's PCM on this CPU: the canonical example's hash.
RUN --mount=type=cache,id=kuula-miyoo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=kuula-miyoo-target,target=/cache/target \
    set -e; \
    cargo test --release --locked --target armv7-unknown-linux-gnueabihf --no-run \
        -p kuula-core -p kuula-lua -p kuula-cli --no-default-features \
        --lib --test determinism --test omt 2>&1 | tee /tmp/test-build.log; \
    sed -n 's/.*Executable .*(\(.*\))$/\1/p' /tmp/test-build.log | while read exe; do \
        cp "$exe" /out/tests/; done

FROM scratch AS artifacts
COPY --from=build /out /
