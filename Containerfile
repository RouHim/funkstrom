# # # # # # # # # # # # # # # # # # # #
# Application Builder
# # # # # # # # # # # # # # # # # # # #
FROM ghcr.io/rust-cross/rust-musl-cross:x86_64-musl AS builder

WORKDIR /app

# Copy source code
COPY Cargo.toml Cargo.lock ./
COPY openapi.yaml ./
COPY src/ src/
COPY templates/ templates/

# Build the application
RUN cargo build --release

# Copy the binary to a known location for the final stage
RUN find /app/target -name "funkstrom" -type f -executable | head -1 | xargs -I{} cp {} /app/funkstrom-binary

# # # # # # # # # # # # # # # # # # # #
# Runtime
# # # # # # # # # # # # # # # # # # # #
FROM alpine

RUN apk add --no-cache ffmpeg && \
    ln -s /usr/bin/ffmpeg /ffmpeg && \
    mkdir -p /app /music && \
    chown -R 1000:1000 /app /music

WORKDIR /app

# Copy the compiled application binary
COPY --chmod=755 --from=builder /app/funkstrom-binary /funkstrom

# Copy templates for runtime rendering
COPY --chmod=644 --from=builder /app/templates/ /templates/

# Copy baked-in default configuration
COPY container-data/default-config.toml /config.toml

EXPOSE 8284

USER 1000

ENTRYPOINT ["/funkstrom", "--config", "/config.toml"]
