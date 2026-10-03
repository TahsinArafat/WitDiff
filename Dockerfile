FROM rust:1.90-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
COPY . .
RUN cargo build --workspace

ENTRYPOINT ["cargo", "run", "-p", "witdiff", "--"]
CMD ["--help"]
