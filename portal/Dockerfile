# syntax=docker/dockerfile:1

# ---- deps: install dependencies with full devDependencies (needed to build) ----
FROM node:22-alpine AS deps
WORKDIR /app
COPY package.json package-lock.json* ./
RUN npm ci

# ---- build: compile the Next.js production build ----
FROM node:22-alpine AS build
WORKDIR /app
ENV NODE_ENV=production
# NEXT_PUBLIC_* vars are inlined into the client/server bundle at build
# time (app/page.tsx is statically prerendered), so this must be passed as
# a --build-arg (compose.yml's portal.build.args does this) -- setting it
# only under compose's `environment:` at runtime has no effect.
ARG NEXT_PUBLIC_GRAFANA_URL
ENV NEXT_PUBLIC_GRAFANA_URL=$NEXT_PUBLIC_GRAFANA_URL
COPY --from=deps /app/node_modules ./node_modules
COPY . .
RUN npm run build

# ---- runtime: minimal image running the Next.js "standalone" server ----
FROM node:22-alpine AS runtime
WORKDIR /app
ENV NODE_ENV=production
ENV PORT=3000
ENV HOSTNAME=0.0.0.0

# Run as an unprivileged user.
RUN addgroup --system --gid 1001 nodejs \
  && adduser --system --uid 1001 nextjs

# next.config.ts sets output:"standalone", so .next/standalone contains a
# minimal server.js plus only the node_modules actually required at runtime;
# static assets still need to be copied alongside it.
COPY --from=build --chown=nextjs:nodejs /app/.next/standalone ./
COPY --from=build --chown=nextjs:nodejs /app/.next/static ./.next/static
COPY --from=build --chown=nextjs:nodejs /app/public ./public

USER nextjs

EXPOSE 3000

CMD ["node", "server.js"]
