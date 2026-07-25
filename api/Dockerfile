# Dependency layer first: Cargo.toml/lock change far less often than src/, so a
# code-only edit reuses the compiled dependency graph instead of rebuilding it.
FROM rust:1.91-slim AS build
WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends pkg-config \
 && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
 && cargo build --release \
 && rm -rf src

COPY src ./src
# Cargo skips a rebuild when only mtime changed; touch forces the real source in.
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
# rustls means no OpenSSL runtime dependency to carry.
COPY --from=build /build/target/release/map-api /usr/local/bin/map-api
EXPOSE 8000
CMD ["map-api"]
