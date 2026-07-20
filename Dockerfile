FROM node:22-bookworm-slim AS dependencies
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund

FROM dependencies AS build
WORKDIR /app
COPY . .
RUN npm run build

FROM node:22-bookworm-slim AS app
ENV NODE_ENV=production
WORKDIR /app
COPY --from=dependencies /app/node_modules ./node_modules
COPY --from=build /app/.output ./.output
COPY --from=build /app/drizzle ./drizzle
COPY --from=build /app/package.json ./package.json
RUN mkdir -p /app/data && chown -R node:node /app
USER node
EXPOSE 3000
CMD ["sh", "-c", "node .output/server/index.mjs"]

FROM node:22-bookworm-slim AS manager
ENV NODE_ENV=production
WORKDIR /app
COPY --from=dependencies /app/node_modules ./node_modules
COPY --from=build /app/.output/manager ./manager
EXPOSE 8788
CMD ["node", "manager/index.mjs"]

FROM node:22-bookworm-slim AS reconciler
ENV NODE_ENV=production
WORKDIR /app
COPY --from=dependencies /app/node_modules ./node_modules
COPY --from=build /app/.output/reconciler ./reconciler
COPY --from=build /app/drizzle ./drizzle
RUN mkdir -p /app/data && chown -R node:node /app
USER node
CMD ["node", "reconciler/index.mjs"]
