# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.95
ARG DEBIAN_VERSION=bookworm

FROM rust:${RUST_VERSION}-${DEBIAN_VERSION} AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    build-essential \
    ca-certificates \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY img ./img
COPY *.sql ./
COPY *.sh ./

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release && \
    cp target/release/jas-min /usr/local/bin/jas-min

FROM debian:${DEBIAN_VERSION}-slim AS runtime

LABEL org.opencontainers.image.title="JAS-MIN"
LABEL org.opencontainers.image.description="JSON AWR & Statspack Miner - Oracle Database performance analysis tool"
LABEL org.opencontainers.image.source="https://github.com/ora600pl/jas-min"
LABEL org.opencontainers.image.licenses="AGPL-3.0"

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    tzdata \
    && rm -rf /var/lib/apt/lists/*

RUN useradd \
    --create-home \
    --home-dir /home/jasmin \
    --shell /usr/sbin/nologin \
    --uid 10001 \
    jasmin

COPY --from=builder /usr/local/bin/jas-min /usr/local/bin/jas-min

ENV JASMIN_HOME=/jasmin/home

RUN mkdir -p /jasmin/home /work && \
    chown -R jasmin:jasmin /jasmin /work

VOLUME ["/jasmin/home", "/work"]

WORKDIR /work

USER jasmin

ENTRYPOINT ["jas-min"]
CMD ["--help"]