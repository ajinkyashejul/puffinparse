# syntax=docker/dockerfile:1
# PuffinParse gateway image: `puffinparse serve` on a distroless base.
#
#   docker build -t puffinparse .
#   docker run --rm -p 4000:4000 \
#     -v $PWD/examples/server/puffinparse.toml:/etc/puffinparse/puffinparse.toml:ro \
#     -e PUFFINPARSE_MASTER_KEY -e PUFFINPARSE_KEY_BILLING -e PUFFINPARSE_KEY_RESEARCH \
#     -e REDUCTO_API_KEY -e EXTEND_API_KEY -e LLAMA_API_KEY \
#     puffinparse
#
# See docs/SERVER.md. The image also carries the full CLI (`docker run puffinparse parse ...`).

FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p puffinparse-cli \
    && cp target/release/puffinparse /puffinparse

# distroless/cc: glibc + libgcc + CA certificates, no shell, runs as uid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /puffinparse /usr/local/bin/puffinparse
EXPOSE 4000
ENTRYPOINT ["/usr/local/bin/puffinparse"]
# Fails closed: without a mounted config the container exits instead of serving an open gateway.
CMD ["serve", "--host", "0.0.0.0", "--config", "/etc/puffinparse/puffinparse.toml"]
