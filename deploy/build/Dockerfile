# syntax=docker/dockerfile:1
# The release build of Yulius for CD (ADR-0430), run by scripts/cd/build.sh with this repository as
# its context: `cargo build --release --locked -p yulius` on Ubuntu 24.04, so the binary runs on
# 24.04 and every later Ubuntu (glibc 2.39 or newer), with the toolchain rust-toolchain.toml names.
# The crate registry and the target directory are BuildKit cache mounts kept by the builder's Docker
# engine between builds; the result is the binary alone.
# ubuntu:24.04, pinned by its digest (2026-11-21).
FROM ubuntu:24.04@sha256:9c7f200c07e3d4a03e7249d2c16bf66a51120fb876aa2242cf31451412543289 AS build
RUN apt-get update -q && apt-get install -y -q --no-install-recommends \
      build-essential ca-certificates curl git pkg-config \
    && rm -rf /var/lib/apt/lists/*
# rustup-init pinned by its digest (static.rust-lang.org/rustup/archive/1.29.3).
ARG RUSTUP_VERSION=1.29.3
ARG RUSTUP_SHA256=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55
ARG RUST_TOOLCHAIN
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN test -n "$RUST_TOOLCHAIN" \
    && curl -fsSL -o /tmp/rustup-init \
      "https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/x86_64-unknown-linux-gnu/rustup-init" \
    && echo "$RUSTUP_SHA256  /tmp/rustup-init" | sha256sum -c - \
    && chmod +x /tmp/rustup-init \
    && /tmp/rustup-init -y -q --no-modify-path --profile minimal --default-toolchain "$RUST_TOOLCHAIN" \
    && rm /tmp/rustup-init
WORKDIR /src
COPY . .
ARG YULIUS_BUILD_COMMIT
# RUSTUP_TOOLCHAIN overrides rust-toolchain.toml: the build needs none of its extra components and
# targets (clippy, rust-analyzer, macOS).
RUN --mount=type=cache,id=yulius-cd-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=yulius-cd-target,target=/target \
    test -n "$YULIUS_BUILD_COMMIT" \
    && RUSTUP_TOOLCHAIN="$RUST_TOOLCHAIN" CARGO_TARGET_DIR=/target \
      cargo build --release --locked -p yulius \
    && mkdir -p /out && cp /target/release/yulius /out/yulius

FROM scratch
COPY --from=build /out/yulius /yulius
