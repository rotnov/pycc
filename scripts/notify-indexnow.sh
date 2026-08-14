#!/usr/bin/env sh
set -eu

unset CDPATH
repo_root=$(cd -- "$(dirname -- "$0")/.." && pwd)
canonical='https://rotnov.github.io/pycc/'
key='3361fe03d0f44ab7cdbb1a3ce1461821'
key_location="${canonical}${key}.txt"
endpoint=${INDEXNOW_ENDPOINT:-https://api.indexnow.org/indexnow}
sitemap=${INDEXNOW_SITEMAP:-"$repo_root/site/sitemap.xml"}
retry_count=${INDEXNOW_RETRY_COUNT:-3}
connect_timeout=${INDEXNOW_CONNECT_TIMEOUT_SECONDS:-10}
max_time=${INDEXNOW_MAX_TIME_SECONDS:-30}

case "$retry_count" in
  ''|*[!0-9]*)
    echo "INDEXNOW_RETRY_COUNT must be a non-negative integer" >&2
    exit 2
    ;;
esac
case "$connect_timeout" in
  ''|*[!0-9]*|0)
    echo "INDEXNOW_CONNECT_TIMEOUT_SECONDS must be a positive integer" >&2
    exit 2
    ;;
esac
case "$max_time" in
  ''|*[!0-9]*|0)
    echo "INDEXNOW_MAX_TIME_SECONDS must be a positive integer" >&2
    exit 2
    ;;
esac

payload=$(python3 - "$sitemap" "$canonical" "$key" "$key_location" <<'PY'
import json
from pathlib import Path
import sys
from urllib.parse import unquote, urlsplit
import xml.etree.ElementTree as ET

sitemap_path = Path(sys.argv[1])
canonical = sys.argv[2]
key = sys.argv[3]
key_location = sys.argv[4]
namespace = {"s": "http://www.sitemaps.org/schemas/sitemap/0.9"}

canonical_parts = urlsplit(canonical)
if canonical_parts.scheme != "https" or not canonical_parts.hostname:
    raise SystemExit("IndexNow canonical URL must be an absolute HTTPS URL")

try:
    root = ET.parse(sitemap_path).getroot()
except (OSError, ET.ParseError) as error:
    raise SystemExit(f"Could not read IndexNow sitemap: {error}") from error

entries = root.findall("s:url", namespace)
if not entries:
    raise SystemExit("IndexNow sitemap contains no canonical URLs")
urls = []
for entry in entries:
    locations = entry.findall("s:loc", namespace)
    if len(locations) != 1:
        raise SystemExit(
            "IndexNow sitemap URL entry must contain exactly one loc"
        )
    urls.append((locations[0].text or "").strip())
if len(urls) > 10_000:
    raise SystemExit("IndexNow batch exceeds the 10,000 URL protocol limit")
if len(urls) != len(set(urls)):
    raise SystemExit("IndexNow sitemap contains duplicate canonical URLs")

for url in urls:
    parts = urlsplit(url)
    decoded_path = parts.path
    for _ in range(5):
        next_path = unquote(decoded_path)
        if next_path == decoded_path:
            break
        decoded_path = next_path
    else:
        raise SystemExit(
            f"IndexNow sitemap URL has excessive percent encoding: {url}"
        )
    has_dot_segment = any(
        segment in {".", ".."} for segment in decoded_path.split("/")
    )
    if (
        parts.scheme != canonical_parts.scheme
        or parts.hostname != canonical_parts.hostname
        or parts.port != canonical_parts.port
        or parts.username is not None
        or parts.password is not None
        or "\\" in decoded_path
        or not decoded_path.startswith(canonical_parts.path)
        or has_dot_segment
        or parts.query
        or parts.fragment
    ):
        raise SystemExit(
            f"IndexNow sitemap URL is outside the verified project path: {url}"
        )

print(
    json.dumps(
        {
            "host": canonical_parts.hostname,
            "key": key,
            "keyLocation": key_location,
            "urlList": urls,
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )
)
PY
)

if [ "${INDEXNOW_DRY_RUN:-}" = "1" ]; then
  printf '%s\n' "$payload"
  exit 0
fi

sitemap_sha256=$(printf '%s' "$payload" | sha256sum | cut -d' ' -f1)
url_count=$(printf '%s' "$payload" | python3 -c "import json,sys; print(len(json.load(sys.stdin)['urlList']))")
timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)
endpoint_host=$(printf '%s' "$endpoint" | sed 's|^[a-z]*://||; s|/.*||')
deployed_commit=${GITHUB_SHA:-unknown}
retry_config="retries=${retry_count}"

# Explicit per-attempt loop replaces opaque curl --retry.
#
# The IndexNow protocol defines direct 200/202 as the only accepted
# receipt states.  Redirects (3xx) are fail-closed: the notifier does
# not follow them, because a redirected request may not carry the
# original JSON payload and the final 200 would be a false delivery
# claim (issue #205: "a redirect can erase the payload and still
# produce a successful notifier result").
#
# Each attempt is recorded so that ambiguous response-loss states
# (body sent but no accepted response) are preserved in the audit
# trail.  A later 200/202 after an ambiguous body-bearing attempt
# is classified as *_after_ambiguous_retry with
# possible_duplicate_delivery=true, because the earlier attempt may
# have been accepted before its response was lost.
#
# Per the IndexNow protocol:
#   200 = URL set submitted successfully
#   202 = URL set received; key validation pending
#   400 = Invalid request format
#   403 = Invalid/unavailable key
#   422 = URL/host/key-scope mismatch
#   429 = Too many requests
# Receipt (200/202) is not proof of crawl or indexing.
max_attempts=$((retry_count + 1))
attempt=0
final_http_code="000"
final_curl_exit=0
body_sent_any_attempt=false
ambiguous_attempt_count=0
should_stop=false

while [ "$attempt" -lt "$max_attempts" ] && [ "$should_stop" = "false" ]; do
  attempt=$((attempt + 1))
  curl_exit=0
  write_out=$(curl \
    --disable \
    --connect-timeout "$connect_timeout" \
    --data-binary "$payload" \
    --header 'Content-Type: application/json; charset=utf-8' \
    --max-time "$max_time" \
    --request POST \
    --show-error \
    --silent \
    --output /dev/null \
    --write-out '%{http_code} %{size_upload}' \
    "$endpoint" 2>/dev/null) || curl_exit=$?

  attempt_http=$(printf '%s' "$write_out" | cut -d' ' -f1)
  attempt_bytes=$(printf '%s' "$write_out" | cut -d' ' -f2)
  if [ -z "$attempt_http" ]; then
    attempt_http="000"
  fi
  if [ -z "$attempt_bytes" ]; then
    attempt_bytes="0"
  fi

  # Track whether the request body was transmitted on this attempt.
  # size_upload > 0 means bytes crossed the wire to the server.
  attempt_body_sent=false
  if [ "$attempt_bytes" -gt 0 ] 2>/dev/null; then
    attempt_body_sent=true
    body_sent_any_attempt=true
  fi

  final_http_code="$attempt_http"
  final_curl_exit="$curl_exit"

  case "$attempt_http" in
    200|202)
      # Accepted response -- stop immediately.
      should_stop=true
      ;;
    3*)
      # Redirect: fail-closed, do not follow or retry.
      # IndexNow does not define redirect as receipt.
      should_stop=true
      ;;
    400|403|422)
      # Permanent client errors: do not retry.
      should_stop=true
      ;;
    429|5*)
      # Transient server errors: retry if attempts remain.
      # Not ambiguous -- server sent an explicit response.
      ;;
    ""|000)
      # No HTTP response (network error, timeout, TLS error).
      # If the body was sent, this attempt is ambiguous: the server
      # may have received and processed the payload before the
      # response was lost.
      if [ "$attempt_body_sent" = "true" ]; then
        ambiguous_attempt_count=$((ambiguous_attempt_count + 1))
      fi
      ;;
    *)
      # Unknown HTTP status: do not retry.
      should_stop=true
      ;;
  esac
done

# Classify the outcome into delivery_state, accepted_class, and
# failure_class.  The classification is derived from the final HTTP
# response code, the curl exit status, and the per-attempt history,
# never from response bodies or headers that might contain sensitive
# data.
#
# Delivery state machine (issue #205):
#   submitted: direct 200 with no earlier ambiguous attempt
#   key_validation_pending: direct 202 with no earlier ambiguous attempt
#   submitted_after_ambiguous_retry: 200 after an earlier ambiguous attempt
#   key_validation_pending_after_ambiguous_retry: 202 after earlier ambiguity
#   delivery_unknown_after_payload_write: no accepted response, body was sent
#   failed_before_payload_write: no accepted response, body was never sent
#   redirect_rejected: 3xx response (fail-closed, no redirect following)
#   http_rejected: 4xx/5xx response (server explicitly rejected)
possible_duplicate_delivery=false
response_code="$final_http_code"

if [ -z "$response_code" ] || [ "$response_code" = "000" ]; then
  response_code="none"
  accepted_class="failed"
  if [ "$body_sent_any_attempt" = "true" ]; then
    delivery_state="delivery_unknown_after_payload_write"
  else
    delivery_state="failed_before_payload_write"
  fi
  case "$final_curl_exit" in
    6)   failure_class="dns_error" ;;
    7)   failure_class="connect_error" ;;
    28)  failure_class="timeout" ;;
    35|52|56|60) failure_class="tls_error" ;;
    *)   failure_class="network_error" ;;
  esac
else
  case "$response_code" in
    200)
      failure_class="none"
      if [ "$ambiguous_attempt_count" -gt 0 ]; then
        delivery_state="submitted_after_ambiguous_retry"
        accepted_class="submitted_after_ambiguous_retry"
        possible_duplicate_delivery=true
      else
        delivery_state="submitted"
        accepted_class="submitted"
      fi
      ;;
    202)
      failure_class="none"
      if [ "$ambiguous_attempt_count" -gt 0 ]; then
        delivery_state="key_validation_pending_after_ambiguous_retry"
        accepted_class="key_validation_pending_after_ambiguous_retry"
        possible_duplicate_delivery=true
      else
        delivery_state="key_validation_pending"
        accepted_class="key_validation_pending"
      fi
      ;;
    3*)
      accepted_class="failed"
      failure_class="redirect_rejected"
      delivery_state="redirect_rejected"
      ;;
    400)
      accepted_class="failed"
      failure_class="bad_request"
      delivery_state="http_rejected"
      ;;
    403)
      accepted_class="failed"
      failure_class="forbidden"
      delivery_state="http_rejected"
      ;;
    422)
      accepted_class="failed"
      failure_class="scope_mismatch"
      delivery_state="http_rejected"
      ;;
    429)
      accepted_class="failed"
      failure_class="rate_limited"
      delivery_state="http_rejected"
      ;;
    5*)
      accepted_class="failed"
      failure_class="http_error_${response_code}"
      delivery_state="http_rejected"
      ;;
    *)
      accepted_class="failed"
      failure_class="http_error_${response_code}"
      delivery_state="http_rejected"
      ;;
  esac
fi

# Emit the structured one-line result record on every terminal path.
# Never log the full server response body, credentials, or runner state.
# The IndexNow verification key is public by protocol design.
result_line="IndexNow result: timestamp=${timestamp} endpoint_host=${endpoint_host} method=POST url_count=${url_count} sitemap_sha256=${sitemap_sha256} deployed_commit=${deployed_commit} http_status=${response_code} accepted_class=${accepted_class} failure_class=${failure_class} delivery_state=${delivery_state} attempt_count=${attempt} possible_duplicate_delivery=${possible_duplicate_delivery} ${retry_config} curl_exit=${final_curl_exit}"
printf '%s\n' "$result_line"

# Publish a concise job summary so a reader can identify the trusted
# commit, URL count, sitemap digest, response class, and delivery
# outcome without downloading raw logs.
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    printf '### IndexNow notification\n\n'
    printf '| Field | Value |\n|---|---|\n'
    printf '| Timestamp (UTC) | %s |\n' "$timestamp"
    printf '| Endpoint host | %s |\n' "$endpoint_host"
    printf '| URL count | %s |\n' "$url_count"
    printf '| Sitemap SHA-256 | `%s` |\n' "$sitemap_sha256"
    printf '| Deployed commit | `%s` |\n' "$deployed_commit"
    printf '| HTTP status | %s |\n' "$response_code"
    printf '| Delivery state | %s |\n' "$delivery_state"
    printf '| Accepted class | %s |\n' "$accepted_class"
    printf '| Failure class | %s |\n' "$failure_class"
    printf '| Attempt count | %s |\n' "$attempt"
    printf '| Possible duplicate delivery | %s |\n' "$possible_duplicate_delivery"
    printf '| Retry config | %s |\n' "$retry_config"
    printf '| curl exit | %s |\n\n' "$final_curl_exit"
    printf 'Receipt (200/202) proves the endpoint accepted the batch, '
    printf '**not** that URLs were crawled or indexed. '
    printf 'Crawl and indexing states are separate observations.\n'
    if [ "$possible_duplicate_delivery" = "true" ]; then
      printf 'A prior attempt sent the payload but received no accepted '
      printf 'response before the final 200/202. The earlier attempt may '
      printf 'have been accepted before its response was lost; '
      printf 'this is an at-least-once delivery, not exactly-once.\n'
    fi
  } >> "$GITHUB_STEP_SUMMARY"
fi

case "$response_code" in
  200|202)
    ;;
  *)
    echo "IndexNow endpoint returned unexpected HTTP status: $response_code (delivery_state=${delivery_state}, failure_class=${failure_class})" >&2
    exit 1
    ;;
esac
