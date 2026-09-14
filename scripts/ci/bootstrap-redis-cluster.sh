#!/usr/bin/env bash
set -euo pipefail
if [[ ${1:-} == --help ]]; then
  echo 'Usage: bootstrap-redis-cluster.sh (Linux Docker and GITHUB_ENV required)'
  exit 0
fi
: "${GITHUB_ENV:?}"
for port in 7000 7001 7002 7003 7004 7005; do
  docker run --detach --name "turnloop-redis-$port" --network host redis:8 \
    redis-server --bind 127.0.0.1 --port "$port" --cluster-enabled yes \
    --cluster-config-file nodes.conf --cluster-node-timeout 5000 --appendonly no \
    --cluster-announce-ip 127.0.0.1 --cluster-announce-port "$port" \
    --cluster-announce-bus-port "$((port + 10000))"
  ready=false
  for _ in {1..60}; do
    if docker exec "turnloop-redis-$port" redis-cli -p "$port" ping; then
      ready=true
      break
    fi
    sleep 1
  done
  [[ $ready == true ]] || { echo "Redis $port failed" >&2; exit 1; }
done
docker exec turnloop-redis-7000 redis-cli --cluster create \
  127.0.0.1:7000 127.0.0.1:7001 127.0.0.1:7002 \
  127.0.0.1:7003 127.0.0.1:7004 127.0.0.1:7005 --cluster-replicas 1 --cluster-yes
ready=false
for _ in {1..60}; do
  if docker exec turnloop-redis-7000 redis-cli -p 7000 cluster info | tr -d '\r' | rg '^cluster_state:ok$'; then
    ready=true
    break
  fi
  sleep 1
done
[[ $ready == true ]] || { echo 'Redis cluster not healthy' >&2; exit 1; }
docker exec turnloop-redis-7000 redis-cli -c -p 7000 SET '{ci}:probe' cluster-ready
docker exec turnloop-redis-7001 redis-cli -c -p 7001 GET '{ci}:probe' | rg '^cluster-ready$'
printf '%s\n' 'TURNLOOP_TEST_REDIS_CLUSTER_URLS=redis://127.0.0.1:7000,redis://127.0.0.1:7001,redis://127.0.0.1:7002' >> "$GITHUB_ENV"
