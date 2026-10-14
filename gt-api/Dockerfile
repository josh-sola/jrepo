# syntax=docker/dockerfile:1
FROM oven/bun:1.3.14-debian AS deps
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile --production

FROM oven/bun:1.3.14-debian

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

ENV NODE_ENV=production \
    GT_API_CONFIG=/etc/gt-api/config.json \
    GT_API_DATA_DIR=/data

WORKDIR /app
COPY --from=deps /app/node_modules ./node_modules
COPY package.json tsconfig*.json ./
COPY src ./src

USER bun
EXPOSE 7434
CMD ["bun", "src/index.ts"]
