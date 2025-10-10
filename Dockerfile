# ======================
# Build stage
# ======================
# use nightly since edition2024 is required
FROM rustlang/rust:nightly-slim AS builder

WORKDIR /app

# Install dependencies needed for building Rust crates with native deps (protobuf, openssl, etc.)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    pkg-config \
    libssl-dev \
    libprotobuf-dev \
    protobuf-compiler \
    build-essential \
    g++ \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests and source
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Build
RUN cargo build --release --bin monitor_with_filters

# ======================
# Runtime stage
# ======================
FROM debian:bookworm-slim AS runtime

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/monitor_with_filters /usr/local/bin/monitor_with_filters

ENTRYPOINT ["monitor_with_filters"]
