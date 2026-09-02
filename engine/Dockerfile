FROM lukemathwalker/cargo-chef:latest-rust-1.88 AS chef
WORKDIR /build

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin kaveon-server --bin kaveon

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/kaveon-server /usr/local/bin/kaveon-server
COPY --from=builder /build/target/release/kaveon /usr/local/bin/kaveon
RUN mkdir -p /etc/kaveon /etc/kaveon/catalogs /data
EXPOSE 8080
ENTRYPOINT ["kaveon-server"]
CMD ["/etc/kaveon/config.toml"]
