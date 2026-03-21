# ── Stage 1: build Next.js ────────────────────────────────────────────────────
FROM node:20-alpine AS builder

RUN npm install -g pnpm@10

WORKDIR /app

# Copy workspace manifests
COPY pnpm-workspace.yaml pnpm-lock.yaml .npmrc ./
COPY package.json ./
COPY packages/types/package.json ./packages/types/
COPY apps/loomx-web/package.json ./apps/loomx-web/

RUN pnpm install --frozen-lockfile

# Copy source
COPY packages/types ./packages/types
COPY apps/loomx-web ./apps/loomx-web

# Build-time env vars baked into the Next.js bundle.
# Pass these via --build-arg when building the Docker image.
# API_URL must be the public HTTPS URL of the loomx-api Container App.
ARG API_URL
ARG AZURE_CLIENT_ID
ARG AZURE_TENANT_ID
ARG WEB_URL

ENV API_URL=${API_URL}
ENV AZURE_CLIENT_ID=${AZURE_CLIENT_ID}
ENV AZURE_TENANT_ID=${AZURE_TENANT_ID}
ENV WEB_URL=${WEB_URL}

RUN pnpm --filter loomx-web build

# ── Stage 2: production image ─────────────────────────────────────────────────
# next.config.ts must have output: 'standalone' for this to work.
FROM node:20-alpine AS runner

WORKDIR /app

ENV NODE_ENV=production
ENV PORT=3000
ENV HOSTNAME=0.0.0.0

# Next.js standalone output is a self-contained server
COPY --from=builder /app/apps/loomx-web/.next/standalone ./
COPY --from=builder /app/apps/loomx-web/.next/static ./apps/loomx-web/.next/static
COPY --from=builder /app/apps/loomx-web/public ./apps/loomx-web/public

EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD wget -qO- http://localhost:3000 || exit 1

# The standalone server.js is at the root of the standalone output
CMD ["node", "apps/loomx-web/server.js"]
