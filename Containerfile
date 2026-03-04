# # # # # # # # # # # # # # # # # # # #
# FFmpeg Downloader
# # # # # # # # # # # # # # # # # # # #
FROM alpine AS ffmpeg-downloader

ARG TARGETARCH

# Download and extract static ffmpeg build for the target architecture
RUN apk add --no-cache curl tar xz && \
    if [ "$TARGETARCH" = "arm64" ]; then \
        FFMPEG_ARCH="arm64"; \
    else \
        FFMPEG_ARCH="amd64"; \
    fi && \
    curl -L -o /tmp/ffmpeg.tar.xz "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-${FFMPEG_ARCH}-static.tar.xz" && \
    tar -xJf /tmp/ffmpeg.tar.xz -C /tmp --wildcards '*/ffmpeg' --strip-components=1 && \
    chmod +x /tmp/ffmpeg

# # # # # # # # # # # # # # # # # # # #
# Application Builder
# # # # # # # # # # # # # # # # # # # #
ARG RUST_MUSL_IMAGE=ghcr.io/rust-cross/rust-musl-cross:x86_64-musl
FROM ${RUST_MUSL_IMAGE} AS builder

WORKDIR /app

# Copy source code
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY templates/ templates/

# Build the application
RUN cargo build --release

# Copy the binary to a known location for the final stage
RUN find /app/target -name "funkstrom" -type f -executable | head -1 | xargs -I{} cp {} /app/funkstrom-binary

# # # # # # # # # # # # # # # # # # # #
# Runtime
# # # # # # # # # # # # # # # # # # # #
FROM scratch

WORKDIR /

# Copy ffmpeg static binary
COPY --chmod=755 --from=ffmpeg-downloader /tmp/ffmpeg /ffmpeg

# Copy the compiled application binary
COPY --chmod=755 --from=builder /app/funkstrom-binary /funkstrom

# Copy templates for runtime rendering
COPY --chmod=644 --from=builder /app/templates/ /templates/

# Copy baked-in default configuration
COPY container-data/default-config.toml /config.toml

EXPOSE 8284

USER 1000

ENTRYPOINT ["/funkstrom", "--config", "/config.toml"]
