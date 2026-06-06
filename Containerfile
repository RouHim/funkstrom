# # # # # # # # # # # # # # # # # # # #
# FFmpeg Downloader
# # # # # # # # # # # # # # # # # # # #
FROM alpine:latest AS ffmpeg-downloader

ARG TARGETARCH

# Download and extract static ffmpeg build for the target architecture
RUN apk add --no-cache curl tar xz file && \
    if [ "$TARGETARCH" = "arm64" ]; then \
        FFMPEG_ARCH="arm64"; \
    else \
        FFMPEG_ARCH="amd64"; \
    fi && \
    for i in 1 2 3; do \
        curl -fsSL --retry 2 --retry-delay 5 -A "Mozilla/5.0" -o ffmpeg-release.tar.xz \
            https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-${FFMPEG_ARCH}-static.tar.xz && \
        file ffmpeg-release.tar.xz | grep -q "XZ compressed" && break; \
        echo "Download attempt $i failed, retrying..."; \
        sleep 2; \
    done && \
    tar xf ffmpeg-release.tar.xz && \
    mv ffmpeg-*-${FFMPEG_ARCH}-static/ffmpeg /ffmpeg && \
    chmod +x /ffmpeg && \
    rm -rf ffmpeg-* ffmpeg-release.tar.xz

# # # # # # # # # # # # # # # # # # # #
# Application Builder
# # # # # # # # # # # # # # # # # # # #
FROM ghcr.io/rust-cross/rust-musl-cross:x86_64-musl AS builder-amd64
FROM ghcr.io/rust-cross/rust-musl-cross:aarch64-musl AS builder-arm64
FROM builder-${TARGETARCH:-amd64} AS builder

# Set working directory and create empty directory in one layer
WORKDIR /app
RUN mkdir "/empty_dir"

# Copy source code
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY openapi.yaml ./
COPY src/ src/
COPY templates/ templates/

# Determine the Rust target triple based on architecture
ARG TARGETARCH
RUN if [ "$TARGETARCH" = "arm64" ]; then \
        echo "aarch64-unknown-linux-musl" > /tmp/rust-target; \
    else \
        echo "x86_64-unknown-linux-musl" > /tmp/rust-target; \
    fi

# Build application with musl compatibility
RUN RUST_TARGET=$(cat /tmp/rust-target) && \
    cargo build --profile production --target ${RUST_TARGET}

# # # # # # # # # # # # # # # # # # # #
# Runtime
# # # # # # # # # # # # # # # # # # # #
FROM scratch

ARG TARGETARCH

# Set environment variables in one layer
ENV USER="1000" \
    RUST_LOG="info" \
    FFMPEG_PATH="/ffmpeg"

# Copy the empty directories as writable locations
COPY --chmod=777 --chown=$USER:$USER --from=builder /empty_dir /app
COPY --chmod=777 --chown=$USER:$USER --from=builder /empty_dir /tmp
COPY --chmod=777 --chown=$USER:$USER --from=builder /empty_dir /music

# Copy the built application directly from target directory
# Use wildcard to handle architecture-specific path
COPY --chmod=755 --chown=$USER:$USER --from=builder /app/target/*/production/funkstrom /funkstrom

# Copy ffmpeg static binary
COPY --chmod=755 --chown=$USER:$USER --from=ffmpeg-downloader /ffmpeg /ffmpeg

# Copy baked-in default configuration
COPY --chmod=644 container-data/default-config.toml /config.toml

WORKDIR /app

EXPOSE 8284

USER $USER

ENTRYPOINT ["/funkstrom", "--config", "/config.toml"]
