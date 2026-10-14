# syntax=docker/dockerfile:1
FROM oven/bun:1.3.14-debian AS deps
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile --production

FROM oven/bun:1.3.14-debian AS build
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY . .
RUN bun run build

FROM ghcr.io/astral-sh/uv:0.12.21 AS uv

FROM oven/bun:1.3.14-debian AS bun

FROM debian:trixie-slim

ARG DIFFT_VERSION=0.69.0
ARG DIFFT_SHA256=abd2f42d2afd424312b4862aa7c7bb0320447670ae22fabcc5159db03e2dccbd
ARG NODE_VERSION=24.15.0
ARG NODE_SHA256=f3d5a797b5d210ce8e2cb265544c8e482eaedcb8aa409a8b46da7e8595d0dda0
ARG PNPM_VERSION=12.4.1
ARG BASEDPYRIGHT_VERSION=1.40.2

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential ca-certificates curl git python3 xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL -o /tmp/difft.tgz "https://github.com/Wilfred/difftastic/releases/download/${DIFFT_VERSION}/difft-aarch64-unknown-linux-gnu.tar.gz" \
    && echo "${DIFFT_SHA256}  /tmp/difft.tgz" | sha256sum -c - \
    && tar -xzf /tmp/difft.tgz -C /usr/local/bin difft \
    && rm /tmp/difft.tgz

RUN curl -fsSL -o /tmp/node.tar.xz "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-arm64.tar.xz" \
    && echo "${NODE_SHA256}  /tmp/node.tar.xz" | sha256sum -c - \
    && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 --exclude='*.md' --exclude=LICENSE \
    && rm /tmp/node.tar.xz \
    && npm install -g "pnpm@${PNPM_VERSION}" \
    && npm cache clean --force

COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun
COPY --from=uv /uv /uvx /usr/local/bin/

# Installed at build time so hover never downloads a language server.
RUN UV_TOOL_DIR=/opt/uv-tools UV_TOOL_BIN_DIR=/usr/local/bin UV_PYTHON_INSTALL_DIR=/opt/uv-python \
    uv tool install "basedpyright==${BASEDPYRIGHT_VERSION}"

RUN useradd --create-home --uid 1000 panopticon

# Caches share the data volume with the worktrees so hardlinks work.
ENV NODE_ENV=production \
    PANOPTICON_CONFIG=/etc/panopticon/config.json \
    PANOPTICON_DATA_DIR=/data \
    PNPM_STORE_DIR=/data/cache/pnpm \
    UV_CACHE_DIR=/data/cache/uv \
    UV_PYTHON_INSTALL_DIR=/data/cache/uv-python \
    TURBO_CACHE_DIR=/data/cache/turbo \
    UV_LINK_MODE=hardlink

WORKDIR /app
COPY --from=deps /app/node_modules ./node_modules
COPY --from=build /app/dist ./dist
COPY package.json tsconfig*.json ./
COPY src ./src

USER panopticon
EXPOSE 7433
CMD ["bun", "src/server/index.ts"]
