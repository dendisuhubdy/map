#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${SITE_HOST:?SITE_HOST must be set}" "${REGION_SLUG:=indonesia}"

# TLS must be real, not self-signed: curl without -k is the assertion. A cert that
# failed to issue shows up here as 000 rather than as a passing test.
assert_http_ok "https://$SITE_HOST/healthz" "edge healthz over TLS"

body=$(curl -s --max-time 10 "https://$SITE_HOST/healthz" || echo '')
assert_eq "ok" "$body" "healthz body"

# Plain HTTP must redirect to HTTPS rather than serve.
redir=$(http_code "http://$SITE_HOST/healthz")
if [[ "$redir" == "301" || "$redir" == "308" ]]; then
  echo "  ok: http redirects to https (${redir})"
else
  echo "  FAIL: expected 301/308 from http, got ${redir}" >&2; FAILED=1
fi

assert_eq "200" "$(http_code "https://$SITE_HOST/tiles/${REGION_SLUG}/metadata" 15)" \
  "tiles proxied through the edge"

# Internal service ports must NOT be reachable from outside. They bind to 127.0.0.1 in
# compose and ufw only opens 22/80/443, so this asserts both layers at once.
for p in 2322 5432 8080 8989; do
  assert_eq "000" "$(http_code "http://$SITE_HOST:$p/" 5)" "port $p not exposed publicly"
done

finish
