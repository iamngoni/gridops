# syntax=docker/dockerfile:1.7

FROM node:22-bookworm-slim AS ui-dependencies
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund

FROM ui-dependencies AS ui-build
COPY index.html tsconfig.json vite.config.ts ./
COPY src ./src
RUN npm run build

FROM rust:1.96-slim-bookworm AS rust-build
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential ca-certificates pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build --workspace --release --locked \
    && mkdir -p /app/bin \
    && cp /app/target/release/gridops-api /app/bin/gridops-api \
    && cp /app/target/release/gridops-manager /app/bin/gridops-manager \
    && cp /app/target/release/gridops-reconciler /app/bin/gridops-reconciler

FROM nginxinc/nginx-unprivileged:1.27-alpine AS web
COPY deploy/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=ui-build /app/dist /usr/share/nginx/html
EXPOSE 3000

FROM debian:bookworm-slim AS rust-runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 gridops \
    && mkdir -p /app/data/logs \
    && chown -R gridops:gridops /app
WORKDIR /app

FROM rust-runtime AS api
COPY --from=rust-build /app/bin/gridops-api /usr/local/bin/gridops-api
USER gridops
EXPOSE 8080
CMD ["gridops-api"]

FROM rust-runtime AS reconciler
COPY --from=rust-build /app/bin/gridops-reconciler /usr/local/bin/gridops-reconciler
USER gridops
CMD ["gridops-reconciler"]

FROM rust-runtime AS manager
COPY --from=rust-build /app/bin/gridops-manager /usr/local/bin/gridops-manager
EXPOSE 8788
CMD ["gridops-manager"]
