FROM oven/bun:1 AS build
WORKDIR /app
COPY package.json bun.lock* ./
RUN bun install --frozen-lockfile
COPY . .
RUN bun run build

FROM nginx:alpine
COPY --from=build /app/dist /usr/share/nginx/html/aivcs
COPY nginx.conf.template /etc/nginx/templates/default.conf.template
EXPOSE 8080
ENV PORT=8080
