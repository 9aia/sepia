# syntax=docker/dockerfile:1
#
# Builds the whole workspace: installs deps and builds the TanStack Start web
# app. The same image runs either service (see docker-compose.yml):
#   api:  bun apps/server/src/main.ts        (port 8787)
#   web:  bun apps/web/dist/server/server.js (port 3000)
#
# Agent CLIs (devin / cline) are NOT baked in — mount a host binary or set
# SEPIA_AGENT_<ID>_COMMAND to a command reachable inside the container.

FROM oven/bun:1.3.14 AS build
WORKDIR /app
COPY . .
RUN bun install --frozen-lockfile && cd apps/web && bun run build

FROM oven/bun:1.3.14 AS runtime
WORKDIR /app
ENV NODE_ENV=production
COPY --from=build /app /app
EXPOSE 8787 3000
CMD ["bun", "apps/server/src/main.ts"]
