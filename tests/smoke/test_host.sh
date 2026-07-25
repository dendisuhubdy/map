#!/usr/bin/env bash
# Host readiness. Runs ON THE DROPLET (uses GNU df/stat), not on a laptop.
source "$(dirname "$0")/lib.sh"
: "${DATA_DIR:=/data}"

assert_eq "0" "$(docker info >/dev/null 2>&1; echo $?)" "docker daemon reachable"
assert_eq "0" "$(docker compose version >/dev/null 2>&1; echo $?)" "docker compose plugin present"
assert_eq "0" "$(test -d "$DATA_DIR" && echo 0 || echo 1)" "$DATA_DIR exists"
assert_eq "0" "$(test -w "$DATA_DIR" && echo 0 || echo 1)" "$DATA_DIR writable"

# ~22GB of artifacts plus transient Planetiler scratch; 70G is the working floor.
avail_gb=$(df -BG --output=avail "$DATA_DIR" | tail -1 | tr -dc '0-9')
if (( avail_gb < 70 )); then
  echo "  FAIL: only ${avail_gb}G free on $DATA_DIR, need >= 70G" >&2
  FAILED=1
else
  echo "  ok: ${avail_gb}G free on $DATA_DIR"
fi

# Imports spike well above steady-state; swap keeps the OOM killer away.
swap_mb=$(free -m | awk '/Swap:/ {print $2}')
if (( swap_mb < 2048 )); then
  echo "  FAIL: swap is ${swap_mb}MB, need >= 2048MB" >&2
  FAILED=1
else
  echo "  ok: ${swap_mb}MB swap configured"
fi

# Service ports must never be publicly exposed; ufw is the outer guard.
assert_contains "$(ufw status 2>/dev/null)" "Status: active" "ufw active"

finish
