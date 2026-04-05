# Build stage
FROM rust:1-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release --locked

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    git \
    ripgrep \
    python3 \
    nodejs \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/agent /usr/local/bin/agent

# Non-root user
RUN useradd -m -s /bin/bash agent
USER agent
WORKDIR /home/agent

ENTRYPOINT ["agent"]
