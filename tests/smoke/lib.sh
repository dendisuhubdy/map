#!/usr/bin/env bash
set -euo pipefail

FAILED=0

assert_eq() {
  local expected="$1" actual="$2" msg="${3:-}"
  if [[ "$expected" != "$actual" ]]; then
    echo "  FAIL: ${msg} — expected '${expected}', got '${actual}'" >&2
    FAILED=1
  else
    echo "  ok: ${msg}"
  fi
}

assert_contains() {
  local haystack="$1" needle="$2" msg="${3:-}"
  if [[ "$haystack" != *"$needle"* ]]; then
    echo "  FAIL: ${msg} — '${needle}' not found" >&2
    FAILED=1
  else
    echo "  ok: ${msg}"
  fi
}

# Emits the HTTP status, or 000 if the request never got one. NOT `curl ... || echo 000`:
# curl already writes 000 for %{http_code} on a failed request and *then* exits non-zero,
# so the fallback concatenates into a nonsense '000000' that matches no expectation.
http_code() {
  local code
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time "${2:-10}" "$1" 2>/dev/null) || true
  echo "${code:-000}"
}

assert_http_ok() {
  local url="$1" msg="${2:-}"
  assert_eq "200" "$(http_code "$url")" "${msg} (${url})"
}

assert_file_min_size() {
  local path="$1" min_bytes="$2" msg="${3:-}"
  local size
  # stat(1) differs between GNU (-c) and BSD/macOS (-f); support both so the
  # suite is runnable from a laptop as well as the droplet.
  size=$(stat -c%s "$path" 2>/dev/null || stat -f%z "$path" 2>/dev/null || echo 0)
  if (( size < min_bytes )); then
    echo "  FAIL: ${msg} — ${path} is ${size}B, expected >= ${min_bytes}B" >&2
    FAILED=1
  else
    echo "  ok: ${msg}"
  fi
}

finish() { exit "$FAILED"; }
