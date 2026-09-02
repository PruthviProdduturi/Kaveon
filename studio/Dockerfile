FROM node:22-alpine AS dependencies
WORKDIR /app
ARG NPM_REGISTRY=https://registry.npmjs.org
RUN npm install --global pnpm@10.34.5 --registry=$NPM_REGISTRY
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY studio/package.json studio/package.json
COPY packages/types/package.json packages/types/package.json
RUN --mount=type=cache,id=kaveon-pnpm-store,target=/pnpm/store \
    pnpm config set registry $NPM_REGISTRY \
    && pnpm config set store-dir /pnpm/store \
    && pnpm install --frozen-lockfile

FROM dependencies AS builder
ARG API_URL=http://api:8080
ARG WEB_URL=http://localhost:3000
ENV API_URL=$API_URL
ENV WEB_URL=$WEB_URL
COPY studio studio
COPY packages packages
RUN pnpm --filter kaveon-studio build

FROM node:22-alpine AS runtime
ENV NODE_ENV=production
WORKDIR /app
RUN addgroup --system --gid 1001 nodejs && adduser --system --uid 1001 nextjs
COPY --from=builder --chown=nextjs:nodejs /app/studio/.next/standalone ./
COPY --from=builder --chown=nextjs:nodejs /app/studio/.next/static ./studio/.next/static
COPY --from=builder --chown=nextjs:nodejs /app/studio/public ./studio/public
USER nextjs
EXPOSE 3000
CMD ["node", "studio/server.js"]
