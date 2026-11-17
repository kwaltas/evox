#!/bin/sh
# Restart unhealthy services. Docker's restart policy handles hard crashes;
# this covers "running but not answering".
set -eu

MASTER_URL="http://master:8790/health"
LEADER_URL="http://leaderboard:8791/top?track=ZONA&class=1"
SLEEP=30
FAILS=0

apk add --no-cache curl >/dev/null 2>&1 || true

while true; do
  ok=1
  curl -fsS --max-time 4 "$MASTER_URL" >/dev/null || ok=0
  curl -fsS --max-time 4 "$LEADER_URL" >/dev/null || ok=0

  if [ "$ok" -eq 0 ]; then
    FAILS=$((FAILS + 1))
    echo "$(date -Is) probe failed ($FAILS/3)"
    if [ "$FAILS" -ge 3 ]; then
      echo "restarting services"
      curl -fsS --unix-socket /var/run/docker.sock -X POST http://localhost/containers/evox-master-1/restart >/dev/null || true
      curl -fsS --unix-socket /var/run/docker.sock -X POST http://localhost/containers/evox-leaderboard-1/restart >/dev/null || true
      FAILS=0
    fi
  else
    FAILS=0
  fi
  sleep "$SLEEP"
done
