#!/bin/sh
# Private test instance only. Node selects an ephemeral loopback port.
set -eu
cd "$(dirname "$0")/.."
mkdir -p .tools
case "${1:-}" in
 start)
  if test -f .tools/http-server.pid; then echo 'Private server PID file already exists' >&2; exit 1; fi
  node scripts/private-http-server.mjs "${2:-h1}" >.tools/http-server.log 2>&1 &
  echo "$!" >.tools/http-server.pid
  ;;
 stop)
  if test -f .tools/http-server.pid; then
   private_pid=$(cat .tools/http-server.pid)
   if ps -p "$private_pid" -o command= | rg -q 'node scripts/private-http-server.mjs'; then kill "$private_pid"; fi
   rm .tools/http-server.pid
  fi
  ;;
 *) echo 'usage: scripts/http-server.sh start [h1|h2] | stop' >&2; exit 2 ;;
esac
