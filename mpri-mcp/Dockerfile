# syntax=docker/dockerfile:1.7
# TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY assets ./assets
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked && cp target/release/mprimcp /mprimcp

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --create-home app \
    && mkdir -p /data \
    && chown app:app /data
COPY --from=build /mprimcp /usr/local/bin/mprimcp
USER app
ENV MPRIMCP_DATA_DIR=/data \
    MPRIMCP_BIND=0.0.0.0:8000 \
    RUST_LOG=info
EXPOSE 8000
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/8000 && printf 'GET /health HTTP/1.0\\r\\n\\r\\n' >&3 && grep -q '\"ok\"' <&3"]
ENTRYPOINT ["mprimcp"]
CMD ["serve"]
