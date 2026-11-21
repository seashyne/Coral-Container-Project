// Compose stack templates offered by the Compose dialog.
const COMPOSE_TEMPLATES = {
  "nginx-redis": `services:
  redis:
    image: redis:alpine
    ports:
      - "6379:6379"
  web:
    image: nginx:alpine
    depends_on:
      - redis
    ports:
      - "8080:80"`,
  "alpine-demo": `services:
  cache:
    image: alpine:3.20
    command: /bin/sh -c "echo CACHE_READY && sleep 300"
  app:
    image: alpine:3.20
    depends_on:
      - cache
    environment:
      CACHE_HOST: cache
    command: /bin/sh -c "echo APP_CONNECTED_TO=$CACHE_HOST && cat /etc/hosts && sleep 300"`,
  "minecraft-paper": `services:
  mc:
    image: itzg/minecraft-server
    container_name: mc-server
    ports:
      - "25565:25565"
    environment:
      EULA: "TRUE"
      TYPE: "PAPER"
      VERSION: "LATEST"
      MEMORY: "2G"
    volumes:
      - mc-data:/data
    restart: unless-stopped`,
};
