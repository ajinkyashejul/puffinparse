# syntax=docker/dockerfile:1
# LiteOCR gateway image: `liteocr serve` on a distroless base.
#
#   docker build -t liteocr .
#   docker run --rm -p 4000:4000 \
#     -v $PWD/examples/server/liteocr.toml:/etc/liteocr/liteocr.toml:ro \
#     -e LITEOCR_MASTER_KEY -e LITEOCR_KEY_BILLING -e LITEOCR_KEY_RESEARCH \
#     -e REDUCTO_API_KEY -e EXTEND_API_KEY -e LLAMA_API_KEY \
#     liteocr
#
# See docs/SERVER.md. The image also carries the full CLI (`docker run liteocr parse ...`).

FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p liteocr-cli \
    && cp target/release/liteocr /liteocr

# distroless/cc: glibc + libgcc + CA certificates, no shell, runs as uid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /liteocr /usr/local/bin/liteocr
EXPOSE 4000
ENTRYPOINT ["/usr/local/bin/liteocr"]
# Fails closed: without a mounted config the container exits instead of serving an open gateway.
CMD ["serve", "--host", "0.0.0.0", "--config", "/etc/liteocr/liteocr.toml"]
