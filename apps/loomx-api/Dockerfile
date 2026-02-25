# ── Stage 1: build TypeScript ─────────────────────────────────────────────────
FROM node:20-alpine AS builder

RUN npm install -g pnpm@9

WORKDIR /app

# Copy workspace manifests first (better layer caching)
COPY pnpm-workspace.yaml pnpm-lock.yaml ./
COPY package.json ./
COPY packages/types/package.json ./packages/types/
COPY apps/loomx-api/package.json ./apps/loomx-api/

# Install all workspace deps (needed so @loomx/types resolves)
RUN pnpm install --frozen-lockfile

# Copy source and build
COPY packages/types ./packages/types
COPY apps/loomx-api ./apps/loomx-api

RUN pnpm --filter loomx-api build

# ── Stage 2: production image ─────────────────────────────────────────────────
FROM node:20-alpine AS runner

WORKDIR /app

ENV NODE_ENV=production
ENV PORT=8080

# Copy compiled output and production node_modules only
COPY --from=builder /app/apps/loomx-api/dist ./dist
COPY --from=builder /app/apps/loomx-api/node_modules ./node_modules
COPY --from=builder /app/apps/loomx-api/package.json ./package.json

# Copy schema.sql (used by the setup wizard)
COPY --from=builder /app/apps/loomx-api/schema.sql ./schema.sql

EXPOSE 8080

# Healthcheck — Container Apps uses this to decide when the container is ready
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD wget -qO- http://localhost:8080/api/health || exit 1

CMD ["node", "dist/server.js"]
