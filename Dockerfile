# One image for the whole build: Rust + wasm32-wasip2 and Node/npm.
#
#   docker build -t daq-build .
#   docker run --rm -v "$PWD:/workspace" -u "$(id -u):$(id -g)" -e CI=1 daq-build
FROM rust:1.97-slim-bookworm

RUN rustup target add wasm32-wasip2

# Third-party license notices for the wasm components (run.sh stage)
RUN cargo install --locked --features cli cargo-about@0.9.2

# Copy node from image
COPY --from=node:22-bookworm-slim /usr/local/bin/node /usr/local/bin/node
COPY --from=node:22-bookworm-slim /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
 && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

RUN chmod -R a+rwX "$CARGO_HOME"

ENV HOME=/tmp \
    XDG_CACHE_HOME=/tmp/.cache \
    npm_config_cache=/tmp/npm-cache \
    npm_config_update_notifier=false \
    npm_config_audit=false \
    npm_config_fund=false
RUN mkdir -p /tmp/npm-cache /tmp/.cache && chmod -R a+rwX /tmp/npm-cache /tmp/.cache

# Skip electron as it is not needed here
ENV ELECTRON_SKIP_BINARY_DOWNLOAD=1

WORKDIR /workspace

CMD ["./run.sh", "all"]
