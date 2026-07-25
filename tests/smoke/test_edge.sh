#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${SITE_HOST:?SITE_HOST must be set}" "${REGION_SLUG:=indonesia}"

# TLS must be real, not self-signed: curl without -k is the assertion. A cert that
# failed to issue shows up here as 000 rather than as a passing test.
assert_http_ok "https://$SITE_HOST/healthz" "edge healthz over TLS"

body=$(curl -s --max-time 10 "https://$SITE_HOST/healthz" || echo '')
assert_eq "ok" "$body" "healthz body"

# Plain HTTP must redirect to HTTPS rather than serve.
redir=$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "http://$SITE_HOST/healthz")
if [[ "$redir" == "301" || "$redir" == "308" ]]; then
  echo "  ok: http redirects to https (${redir})"
else
  echo "  FAIL: expected 301/308 from http, got ${redir}" >&2; FAILED=1
fi

code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 15 \
  "https://$SITE_HOST/tiles/${REGION_SLUG}/metadata")
assert_eq "200" "$code" "tiles proxied through the edge"

# Internal service ports must NOT be reachable from outside. These bind to
# 127.0.0.1 in compose and ufw only opens 22/80/443, so this asserts both layers.
for p in 2322 5432 8080 8989; do
  ext=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://$SITE_HOST:$p/" 2>/dev/null || echo 000)
  assert_eq "000" "$ext" "port $p not exposed publicly"
done

finish
