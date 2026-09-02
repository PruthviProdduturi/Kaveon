FROM node:22-alpine
ENV NODE_ENV=production
WORKDIR /app
RUN addgroup --system --gid 1001 nodejs && adduser --system --uid 1001 nextjs
COPY studio/.next/standalone ./
COPY studio/.next/static studio/.next/static
COPY studio/public studio/public
USER nextjs
EXPOSE 3000
CMD ["node", "studio/server.js"]
