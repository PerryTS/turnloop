#!/usr/bin/env bash
set -euo pipefail
if [[ ${1:-} == --help ]]; then
  echo 'Usage: bootstrap-mongo-replica.sh (Linux Docker and GITHUB_ENV required)'
  exit 0
fi
: "${GITHUB_ENV:?}"
for port in 27018 27019 27020; do
  docker run --detach --name "turnloop-mongo-$port" --network host mongo:8 \
    mongod --bind_ip 127.0.0.1 --port "$port" --replSet turnloop-rs
  ready=false
  for _ in {1..90}; do
    if docker exec "turnloop-mongo-$port" mongosh --quiet --port "$port" --eval 'if(!db.adminCommand({ping:1}).ok) quit(1)'; then
      ready=true
      break
    fi
    sleep 1
  done
  [[ $ready == true ]] || { echo "Mongo $port failed" >&2; exit 1; }
done
docker exec turnloop-mongo-27018 mongosh --quiet --port 27018 --eval \
  'const r=rs.initiate({_id:"turnloop-rs",members:[{_id:0,host:"127.0.0.1:27018"},{_id:1,host:"127.0.0.1:27019"},{_id:2,host:"127.0.0.1:27020"}]}); if(!r.ok) throw Error(JSON.stringify(r));'
ready=false
for _ in {1..90}; do
  if docker exec turnloop-mongo-27018 mongosh --quiet --port 27018 --eval \
    'const s=rs.status(); if(s.members.filter(m=>m.state===1).length!==1 || s.members.filter(m=>m.state===2).length!==2) quit(1)'; then
    ready=true
    break
  fi
  sleep 1
done
[[ $ready == true ]] || { echo 'Mongo replica set lacks primary and two secondaries' >&2; exit 1; }
docker exec turnloop-mongo-27018 mongosh --quiet 'mongodb://127.0.0.1:27018,127.0.0.1:27019,127.0.0.1:27020/turnloop?replicaSet=turnloop-rs' --eval \
  'db.ci_probe.insertOne({probe:1},{writeConcern:{w:"majority",wtimeout:10000}}); if(db.ci_probe.countDocuments({probe:1})<1) throw Error("no replicated write");'
printf '%s\n' 'TURNLOOP_TEST_MONGODB_REPLICA_URL=mongodb://127.0.0.1:27018,127.0.0.1:27019,127.0.0.1:27020/turnloop?replicaSet=turnloop-rs' >> "$GITHUB_ENV"
