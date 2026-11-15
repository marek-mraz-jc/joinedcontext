#!/usr/bin/env bash
# Three timed operations drills on throwaway resources in project walk-ops (T-5542).
#
#   tests/drills/walk_ops.sh                       # dry: prints the plan, calls nothing
#   tests/drills/walk_ops.sh --run --server https://portal.dev.joinedcontext.com \
#       --gateway https://api.dev.joinedcontext.com --org joinedcontext.com --owner <you> \
#       --token-file ~/.config/jcctl/token --idm https://idm.dev.joinedcontext.com/realms/dev
#
#   A  a failing pipeline: the drill DataSource is pointed at a path that answers 404; the drill
#      passes when pipeline-health shows walk-fetch failing within --bound-a seconds (one 60 s
#      interval plus the reconcile), then the source is put back and a run succeeds again.
#   B  endpoint abuse (runbook 3): the walk ServiceAccount calls Endpoint walk-read at --rate requests
#      a second for --seconds; the gateway answers 429; the drill passes when the project's
#      Activity names the caller; then the limit is tightened as runbook 3 says and checked.
#   C  a leaked ServiceAccount key (runbook 4): the walk key is treated as leaked, a person issues
#      a new secret on the Portal and its Policy is deleted; the drill passes when the leaked
#      secret and the token minted before are refused (401/403) within --bound-c seconds (OPS-45: 5).
#
# Every write is a Change proposed with the operator's token (jcctl apply/delete -f): a person
# approves it on the Portal's approval page while the script waits. The Portal creates the Keycloak
# client of walk-caller and deletes it with the account (T-5626); its secret is issued only from a
# person's Portal session, so the script asks that person to issue it (and in drill C to issue a
# new one) on the account's card and to paste it on this terminal, where it is read without echo.
# No Keycloak administrator credential is needed.
#
# Safety: nothing shared is stopped or restarted (the reaper re-reads the repository every second,
# T-0177, so runbook 4 restarts no gateway since T-5625); the rate is capped at 20 requests a second, 60 s and
# fewer than 600 calls, the edge's own limit per token and address (context-endpoint limit-count);
# the leaked key exists only in a mode 700 work directory for the drill and is rotated in it.
# No token or secret is printed: only statuses, counts and seconds.
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
template="$here/walk-ops.yaml"
project="walk-ops"
run=0
drills="abc"
server="${JC_SERVER:-}"
gateway=""
token_file="${JC_TOKEN_FILE:-}"
org=""
owner=""
idm=""
rate=20
seconds=25
bound_a=120
bound_c=5
wait_merge=900
poll=5
keep=0
rpm=600
tight_rpm=60

die() { printf 'walk_ops: %s\n' "$1" >&2; exit 2; }

while [ $# -gt 0 ]; do
	case "$1" in
	--run) run=1; shift ;;
	--drill) drills="${2:?}"; shift 2 ;;
	--server) server="${2:?}"; shift 2 ;;
	--gateway) gateway="${2:?}"; shift 2 ;;
	--token-file) token_file="${2:?}"; shift 2 ;;
	--org) org="${2:?}"; shift 2 ;;
	--owner) owner="${2:?}"; shift 2 ;;
	--idm) idm="${2:?}"; shift 2 ;;
	--rate) rate="${2:?}"; shift 2 ;;
	--seconds) seconds="${2:?}"; shift 2 ;;
	--bound-a) bound_a="${2:?}"; shift 2 ;;
	--bound-c) bound_c="${2:?}"; shift 2 ;;
	--wait-merge) wait_merge="${2:?}"; shift 2 ;;
	--poll) poll="${2:?}"; shift 2 ;;
	--keep) keep=1; shift ;;
	-h | --help) sed -n '2,32p' "$0"; exit 0 ;;
	*) die "unknown argument: $1" ;;
	esac
done

case "$drills" in all) drills="abc" ;; esac
[[ "$drills" =~ ^[abc]+$ ]] || die "--drill takes a, b, c, any of them together (ab) or all"
for n in "$rate" "$seconds" "$bound_a" "$bound_c" "$wait_merge" "$poll"; do
	[[ "$n" =~ ^[0-9]+$ ]] || die "a number was expected, not '$n'"
done
# The stated load and no more: a busy node must not carry a drill's flood (T-5542 Security).
if [ "$rate" -lt 1 ] || [ "$rate" -gt 20 ]; then die "--rate is 1..20 requests a second"; fi
if [ "$seconds" -lt 1 ] || [ "$seconds" -gt 60 ]; then die "--seconds is 1..60"; fi
# The edge counts 600 calls a minute per token and address: a flood that reaches it proves its limit, not the gateway's.
if [ $((rate * seconds)) -ge 600 ]; then die "--rate x --seconds must stay under 600 calls (the edge's limit a minute)"; fi
[ "$poll" -ge 1 ] || die "--poll is at least 1 second"
gateway="${gateway:-$server}"

plan() {
	cat <<EOF
walk_ops plan (project $project, drills: $drills; dry, nothing is called — add --run)
setup  open project $project, propose tests/drills/walk-ops.yaml (space walk, DataSource walk-source,
       Pipeline walk-fetch, Endpoints walk-write and walk-read at $rpm/min, ServiceAccount walk-caller,
       Policy walk-caller-read); wait until a person approved every Change
EOF
	case "$drills" in *a*) cat <<EOF ;; esac
A      wait for a good run of walk-fetch; point walk-source at a 404 path; measure until
       pipeline-health shows walk-fetch failing (bound ${bound_a}s) and names the 404; put the URL
       back; measure until a run succeeds again
EOF
	case "$drills" in *b* | *c*) cat <<EOF ;; esac
       wait for the Portal's client $project-walk-caller; a person issues its secret on the
       Portal and pastes it here (no echo), into a mode 700 work directory only
EOF
	case "$drills" in *b*) cat <<EOF ;; esac
B      $rate req/s for ${seconds}s on Endpoint walk-read; count 429s; look for the
       caller in the project's Activity; tighten the limit to $tight_rpm/min (runbook 3) and check
       that the gateway answers with RateLimit-Limit $tight_rpm
EOF
	case "$drills" in *c*) cat <<EOF ;; esac
C      mint a token with the leaked secret; a person issues a new secret on the Portal (runbook 4);
       measure from its access.revoked event until the leaked secret is refused; delete Policy
       walk-caller-read and measure until the earlier token is refused (bound ${bound_c}s each,
       OPS-45); no gateway restart
EOF
	cat <<EOF
down   delete every resource of the file (the Portal removes the client) and project $project$([ "$keep" -eq 1 ] && printf ' (skipped: --keep)')
EOF
}

if [ "$run" -eq 0 ]; then
	plan
	exit 0
fi

[ -n "$server" ] || die "--server <portal url> (or JC_SERVER) is needed with --run"
[ -n "$org" ] || die "--org <organization domain> is needed with --run"
[ -n "$owner" ] || die "--owner <your user name> is needed with --run"
[ -r "${token_file:-/nonexistent}" ] || die "--token-file <a person's Portal token, jcctl login> is needed with --run"
for url in "$server" "$gateway" ${idm:+"$idm"}; do
	[[ "$url" =~ ^https:// || "$url" =~ ^http://(localhost|127\.0\.0\.1)(:|/|$) ]] ||
		die "$url: https only, except on the loopback interface"
done
case "$drills" in *b* | *c*)
	[ -n "$idm" ] || die "drills b and c need --idm <realm issuer url>"
	;;
esac
command -v jcctl >/dev/null || die "jcctl is not on PATH (cargo build -p jcctl in joinedcontext-platform)"
command -v jq >/dev/null || die "jq is not on PATH"

work=$(mktemp -d)
chmod 700 "$work"
trap 'rm -rf "${work:?}"' EXIT
umask 077
# A token goes into curl as a header file, never as an argument another process can read.
printf 'Authorization: Bearer %s\n' "$(cat "$token_file")" >"$work/person.h"

started=$(date +%s)
now() { date +%s; }
say() { printf '[%5ss] %s\n' "$(($(now) - started))" "$1"; }
fail=0
ko() { say "FAIL  $1"; fail=1; }
report=()
record() { report+=("$1"); say "$1"; }

# GET with the person's token; prints the body, then the status on its own last line.
get() { curl -sS --max-time 15 -H @"$work/person.h" -H 'accept: application/json' -w '\n%{http_code}' "$server$1" 2>/dev/null || printf '\n000'; }
status_of() { printf '%s' "${1##*$'\n'}"; }
body_of() { printf '%s' "${1%$'\n'*}"; }

# An Endpoint slug is a 128-bit secret of its own (EP-03): fresh per run, kept when walk-ops exists.
new_slug() { head -c 20 /dev/urandom | base32 | tr -d '=' | tr '[:upper:]' '[:lower:]' | cut -c1-26; }
slug_of() { # $1 Endpoint name: its served slug, else a fresh one
	local answer slug=""
	answer=$(get "/api/v1/projects/$project/endpoints/$1")
	if [ "$(status_of "$answer")" = 200 ]; then slug=$(body_of "$answer" | jq -r '.spec.slug // empty' 2>/dev/null); fi
	if [ -n "$slug" ]; then printf '%s\n' "$slug"; else new_slug; fi
}

render() { # $1 source url, $2 requests per minute
	sed -e "s|@ORG@|$org|g" -e "s|@OWNER@|$owner|g" -e "s|@SOURCE_URL@|$1|g" -e "s|@RPM@|$2|g" \
		-e "s|@READ_SLUG@|$read_slug|g" -e "s|@WRITE_SLUG@|$write_slug|g" \
		"$template" >"$work/walk-ops.yaml"
}

jc() { jcctl "$@" --server "$server" --token-file "$token_file" --project "$project"; }

# Waits until a person approved the Change and the Portal serves (or no longer serves) the resource.
wait_resource() { # $1 plural/name, $2 present|absent, $3 jq test the body must pass (optional)
	local tries=$((wait_merge / poll)) answer code
	say "approve the Change of $1 on $server/projects/$project/changes now"
	while [ "$tries" -gt 0 ]; do
		answer=$(get "/api/v1/projects/$project${1:+/$1}")
		code=$(status_of "$answer")
		if [ "$2" = present ] && [ "$code" = 200 ]; then
			[ -z "${3:-}" ] && return 0
			body_of "$answer" | jq -e "$3" >/dev/null 2>&1 && return 0
		fi
		[ "$2" = absent ] && [ "$code" = 404 ] && return 0
		tries=$((tries - 1))
		sleep "$poll"
	done
	ko "$1 is not $2 after ${wait_merge}s: the Change was not approved or did not deploy"
	return 1
}

health() { # the pipeline-health row of walk-fetch, or nothing
	local answer
	answer=$(get "/api/v1/projects/$project/pipeline-health")
	[ "$(status_of "$answer")" = 200 ] || return 0
	body_of "$answer" | jq -c '.items[]? | select(.pipeline == "walk-fetch")' 2>/dev/null
}

good_url="$server/api/v1/health"
bad_url="$server/api/v1/health/walk-ops-drill-404"

setup() {
	local answer
	answer=$(get "/api/v1/projects/$project")
	if [ "$(status_of "$answer")" != 200 ]; then
		code=$(curl -sS --max-time 15 -o /dev/null -w '%{http_code}' -X POST -H @"$work/person.h" \
			-H 'content-type: application/json' --data "{\"name\":\"$project\",\"displayName\":\"Operations drill\"}" \
			"$server/api/v1/projects" 2>/dev/null) || code=000
		case "$code" in 20*) say "project $project proposed" ;; *) ko "opening project $project answered $code"; return 1 ;; esac
		wait_resource "" present || return 1
	fi
	read_slug=$(slug_of walk-read)
	write_slug=$(slug_of walk-write)
	read_url="$gateway/api/endpoint/$read_slug/ngsi-ld/v1/entities?type=WalkProbe&limit=1"
	render "$good_url" "$rpm"
	jc apply -f "$work/walk-ops.yaml" || { ko "jcctl apply refused part of walk-ops.yaml: read its lines above"; return 1; }
	for r in contextspaces/walk datasources/walk-source endpoints/walk-write endpoints/walk-read \
		pipelines/walk-fetch serviceaccounts/walk-caller policies/walk-caller-read; do
		wait_resource "$r" present || return 1
	done
	say "setup deployed"
}

drill_a() {
	local tries row t0 t_detect t_diag t_back
	tries=$((wait_merge / poll))
	while [ "$tries" -gt 0 ]; do
		row=$(health)
		[ -n "$row" ] && printf '%s' "$row" | jq -e '.state == "ok" or .state == "empty"' >/dev/null && break
		tries=$((tries - 1)); sleep "$poll"
	done
	[ "$tries" -gt 0 ] || { ko "A: walk-fetch never ran clean, so the drill has no baseline"; return; }
	[ "$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' "$bad_url" 2>/dev/null)" = 404 ] ||
		{ ko "A: $bad_url does not answer 404, pick another drill path"; return; }
	render "$bad_url" "$rpm"
	jc apply -f "$work/walk-ops.yaml" || { ko "A: proposing the 404 source was refused"; return; }
	wait_resource datasources/walk-source present ".spec.http.url == \"$bad_url\"" || return
	t0=$(now); t_detect=""; t_diag=""
	tries=$(((bound_a * 3) / poll + 1))
	while [ "$tries" -gt 0 ] && [ -z "$t_diag" ]; do
		row=$(health)
		if [ -n "$row" ] && printf '%s' "$row" | jq -e '.state == "failing" or .failedRuns > 0' >/dev/null; then
			[ -n "$t_detect" ] || t_detect=$(now)
			printf '%s' "$row" | jq -e '(.lastError // "") | test("404")' >/dev/null && t_diag=$(now)
		fi
		[ -n "$t_diag" ] || { tries=$((tries - 1)); sleep "$poll"; }
	done
	if [ -z "$t_detect" ]; then
		ko "A: pipeline-health never showed walk-fetch failing ($((bound_a * 3))s waited)"
	else
		record "A detect $((t_detect - t0))s by pipeline-health (failing)"
		[ $((t_detect - t0)) -le "$bound_a" ] || ko "A: detection took $((t_detect - t0))s, over the ${bound_a}s bound"
		if [ -n "$t_diag" ]; then record "A diagnose $((t_diag - t0))s: lastError names the 404"; else ko "A: lastError never named the 404"; fi
	fi
	render "$good_url" "$rpm"
	jc apply -f "$work/walk-ops.yaml" || { ko "A: proposing the good source back was refused"; return; }
	wait_resource datasources/walk-source present ".spec.http.url == \"$good_url\"" || return
	tries=$(((bound_a * 3) / poll + 1)); t_back=""
	while [ "$tries" -gt 0 ]; do
		row=$(health)
		if [ -n "$row" ] && printf '%s' "$row" | jq -e '.state == "ok" or .state == "empty"' >/dev/null; then t_back=$(now); break; fi
		tries=$((tries - 1)); sleep "$poll"
	done
	if [ -n "$t_back" ]; then record "A recover $((t_back - t0))s: walk-fetch healthy again"; else ko "A: walk-fetch did not recover"; fi
}

client_id="$project-walk-caller"
account_page="$server/projects/$project/settings/service-accounts"
# A secret the person issued on the Portal, read from standard input (the terminal) without echo
# into file $1.
paste_secret() { # $1 file, $2 what to do on the Portal
	local secret=""
	say "$2: on $account_page open walk-caller, 'What this account may do', 'Issue new secret', then paste the secret here"
	IFS= read -rs secret || true
	printf '%s' "$secret" >"$1"
	secret=""
	[ -s "$1" ]
}

# The Portal writes the client once the account is merged (T-5626); until it has, its Issue new
# secret answers that the client does not exist yet, and the person tries again.
make_client() {
	paste_secret "$work/leaked-key" "issue the first secret of $client_id" || { ko "no secret was pasted for $client_id"; return 1; }
	[ -n "$(mint "$work/leaked-key")" ] || { ko "the pasted secret mints no token for $client_id ($(cat "$work/mint.code"))"; return 1; }
	say "client $client_id ready (secret in the work directory only)"
}

mint() { # $1 secret file; prints the access token, or nothing; status in $work/mint.code
	curl -sS --max-time 10 -o "$work/mint.json" -w '%{http_code}' -H 'content-type: application/x-www-form-urlencoded' \
		--data-urlencode "client_id=$client_id" --data-urlencode "client_secret@$1" -d grant_type=client_credentials \
		"$idm/protocol/openid-connect/token" >"$work/mint.code" 2>/dev/null || printf '000' >"$work/mint.code"
	jq -r '.access_token // empty' "$work/mint.json" 2>/dev/null
	rm -f "$work/mint.json"
}

read_slug=""
write_slug=""
read_url=""
# One status per line; a 429 without the gateway's RateLimit-Limit header (the edge sends only
# X-RateLimit-*) is marked "edge", so drill B counts the gateway's 429s alone.
call() {
	curl -sS --max-time 5 -o /dev/null -D - -w 'code %{http_code}\n' -H @"$1" "$read_url" 2>/dev/null | tr -d '\r' |
		awk 'tolower($0) ~ /^ratelimit-limit:/ {g = 1} /^code / {c = $2} END {print (c == "" ? "000" : c) (c == "429" && !g ? " edge" : "")}'
}

drill_b() {
	local token second i t0 t_detect="" t_diag="" answer tries limit
	token=$(mint "$work/leaked-key")
	[ -n "$token" ] || { ko "B: no token for $client_id (token endpoint $(cat "$work/mint.code"))"; return; }
	printf 'Authorization: Bearer %s\n' "$token" >"$work/caller.h"
	t0=$(now)
	: >"$work/codes"
	for ((second = 0; second < seconds; second++)); do
		for ((i = 0; i < rate; i++)); do call "$work/caller.h" >>"$work/codes" & done
		wait
		grep -q '^429$' "$work/codes" && [ -z "$t_detect" ] && t_detect=$(now)
		sleep 1
	done
	record "B flood $(wc -l <"$work/codes") calls: $(sort "$work/codes" | uniq -c | awk '{printf "%s×%s ", $1, $2}')"
	if grep -q '^429 edge$' "$work/codes"; then ko "B: the edge answered 429, so its limit, not the gateway's, was reached"; fi
	if [ -n "$t_detect" ]; then record "B detect $((t_detect - t0))s by the gateway's 429s"; else ko "B: no 429 at $rate req/s for ${seconds}s"; fi
	tries=$((120 / poll + 1))
	while [ "$tries" -gt 0 ]; do
		answer=$(get "/api/v1/projects/$project/activity")
		if [ "$(status_of "$answer")" = 200 ] && body_of "$answer" | grep -q -e "walk-caller" -e "$client_id"; then t_diag=$(now); break; fi
		tries=$((tries - 1)); sleep "$poll"
	done
	if [ -n "$t_diag" ]; then
		record "B diagnose $((t_diag - t0))s: the Activity names walk-caller"
	else
		ko "B: the project's Activity never named the caller walk-caller (runbook 3 Diagnosis has no signal for it, T-5624)"
	fi
	# Runbook 3, Remediation 1: tighten the Endpoint's limit, then check the gateway applies it.
	render "$good_url" "$tight_rpm"
	jc apply -f "$work/walk-ops.yaml" || { ko "B: proposing the tighter limit was refused"; return; }
	wait_resource endpoints/walk-read present ".spec.rateLimits.requestsPerMinute == $tight_rpm" || return
	sleep 2
	limit=$(curl -sS --max-time 5 -o /dev/null -D - -H @"$work/caller.h" "$read_url" 2>/dev/null |
		tr -d '\r' | awk -F': ' 'tolower($1) == "ratelimit-limit" {print $2}')
	if [ "$limit" = "$tight_rpm" ]; then record "B recover $(($(now) - t0))s: walk-read limited to $tight_rpm/min"; else ko "B: RateLimit-Limit is '${limit:-absent}', not $tight_rpm"; fi
}

drill_c() {
	local before code t0 t_secret="" t_token="" tries
	before=$(mint "$work/leaked-key")
	[ -n "$before" ] || { ko "C: no token from the walk key (token endpoint $(cat "$work/mint.code"))"; return; }
	printf 'Authorization: Bearer %s\n' "$before" >"$work/before.h"
	code=$(call "$work/before.h")
	case "$code" in 200) ;; *) say "C: the walk token reads walk-read with $code before the revocation" ;; esac
	# Runbook 4 for a client the Portal made: a person issues a new secret on the Portal. The
	# leaked one is tried every second meanwhile; the bound counts from the access.revoked event.
	say "C: the walk key is leaked now"
	(
		while :; do
			mint "$work/leaked-key" >/dev/null
			case "$(cat "$work/mint.code")" in 400 | 401) now >"$work/refused-at"; exit 0 ;; esac
			sleep 1
		done
	) &
	local watcher=$!
	paste_secret "$work/new-key" "C: issue a new secret of $client_id" || say "C: no new secret pasted; measuring anyway"
	tries=$((bound_c * 4 + 1))
	while [ "$tries" -gt 0 ] && [ ! -s "$work/refused-at" ]; do tries=$((tries - 1)); sleep 1; done
	kill "$watcher" 2>/dev/null; wait "$watcher" 2>/dev/null
	[ -s "$work/refused-at" ] && t_secret=$(cat "$work/refused-at")
	t0=$(body_of "$(get "/api/v1/projects/$project/activity?kind=access.revoked&limit=1")" |
		jq -r '.items[0].time // empty | sub("\\.[0-9]+"; "") | fromdateiso8601' 2>/dev/null)
	[ -n "$t0" ] || { ko "C: the project's Activity has no access.revoked event for the new secret"; t0=$(now); }
	if [ -n "$t_secret" ]; then
		record "C revoke-to-401 $((t_secret - t0))s: the token endpoint refuses the leaked secret"
		[ $((t_secret - t0)) -le "$bound_c" ] || ko "C: the leaked secret was refused only after $((t_secret - t0))s (bound ${bound_c}s)"
	else
		ko "C: the leaked secret still mints tokens"
	fi
	# Runbook 4 step 1: the Policy leaves the repository; the gateway's reaper re-reads it every
	# second (T-0177), so no restart. The bound counts from the merge, not from the approval wait.
	cp "$work/walk-ops.yaml" "$work/all.yaml"
	awk 'BEGIN{RS="---\n"; ORS="---\n"} /kind: Policy/' "$work/all.yaml" >"$work/policy.yaml"
	jc delete -f "$work/policy.yaml" || { ko "C: proposing the Policy deletion was refused"; return; }
	wait_resource policies/walk-caller-read absent || return
	t0=$(now)
	tries=$((bound_c * 4 + 1))
	while [ "$tries" -gt 0 ]; do
		case "$(call "$work/before.h")" in 401 | 403) t_token=$(now); break ;; esac
		tries=$((tries - 1)); sleep 1
	done
	if [ -n "$t_token" ]; then
		record "C policy-to-refusal $((t_token - t0))s: the token minted before is refused at the gateway"
		[ $((t_token - t0)) -le "$bound_c" ] || ko "C: the earlier token was refused only after $((t_token - t0))s (bound ${bound_c}s)"
	else
		ko "C: the token minted before the revocation still reads walk-read"
	fi
}

teardown() {
	[ "$keep" -eq 1 ] && { say "--keep: walk-ops stays; run again without --keep to remove it"; return; }
	render "$good_url" "$rpm"
	jc delete -f "$work/walk-ops.yaml" || ko "down: jcctl delete refused part of walk-ops.yaml"
	for r in policies/walk-caller-read serviceaccounts/walk-caller pipelines/walk-fetch endpoints/walk-read \
		endpoints/walk-write datasources/walk-source contextspaces/walk; do
		wait_resource "$r" absent || true
	done
	case "$(curl -sS --max-time 15 -o /dev/null -w '%{http_code}' -X DELETE -H @"$work/person.h" "$server/api/v1/projects/$project" 2>/dev/null)" in
	20*) wait_resource "" absent || true ;;
	*) ko "down: deleting project $project was refused; remove it on the Portal's project settings" ;;
	esac
}

plan
if setup; then
	case "$drills" in *a*) drill_a ;; esac
	case "$drills" in *b* | *c*) make_client || drills="${drills//[bc]/}" ;; esac
	case "$drills" in *b*) drill_b ;; esac
	case "$drills" in *c*) drill_c ;; esac
fi
teardown

printf '\nwalk_ops report\n'
printf '  %s\n' "${report[@]:-nothing measured}"
if [ "$fail" -eq 0 ]; then
	printf 'walk_ops: every drill passed\n'
	exit 0
fi
printf 'walk_ops: a drill did NOT pass: read the FAIL lines above\n' >&2
exit 1
