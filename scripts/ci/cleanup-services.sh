#!/usr/bin/env bash
set -euo pipefail
if [[ ${1:-} == --help ]]; then
  echo 'Usage: cleanup-services.sh (remove only turnloop CI fixture containers)'
  exit 0
fi
for name in turnloop-redis-{7000..7005} turnloop-mongo-{27018..27020}; do
  if docker container inspect "$name" >/dev/null 2>&1; then
    docker logs "$name"
    docker rm --force "$name"
  fi
done
