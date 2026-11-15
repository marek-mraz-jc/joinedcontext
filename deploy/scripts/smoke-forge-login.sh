#!/usr/bin/env bash
# Sign in to the forge through Keycloak the way a browser does, then read the configuration
# repository and download every application repository (T-1422, T-2601, PF-51, PF-79, AP-78). A login only a person could see broken is the defect
# this catches: the forge's Keycloak button failed on every login for weeks behind a green smoke.
#   scripts/smoke-forge-login.sh <base-url>
# Passwords come from the Secrets keycloak-user-<name>, are sent on stdin and never printed.
set -uo pipefail

base="${1:?usage: smoke-forge-login.sh <base-url>}"
slug="${JC_INSTANCE:-${JC_REALM:-dev}}"
org="${JC_SMOKE_ORG:-$(kubectl get deploy context-gateway -n "$slug" \
	-o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="JC_GATEWAY_ORG_DOMAIN")].value}' 2>/dev/null || true)}"
org="${org:-hel.fi}"
forge_org="${JC_FORGE_ORG:-joinedcontext}"
# The App repositories live in an organization of their own when `appsOrganization` is set
# (T-2969); the bootstrap records it as the runner Secret's `owner`, as scripts/smoke.sh reads it.
apps_org="${JC_FORGE_APPS_ORG:-$(kubectl get secret gitea-runner-registration -n "$slug" -o jsonpath='{.data.owner}' 2>/dev/null | base64 -d 2>/dev/null || true)}"
apps_org="${apps_org:-$forge_org}"
project="${JC_SMOKE_PROJECT:-helsinki}"
# Layout 2 (PF-87, T-2647): the project is a repository of its own, which its readers read; the
# gateway carries JC_GATEWAY_PROJECTS_DIR exactly then. Layout 1: its folder in the configuration.
layout2=$(kubectl get deploy context-gateway -n "$slug" \
	-o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="JC_GATEWAY_PROJECTS_DIR")].value}' 2>/dev/null || true)
if [ -n "${JC_SMOKE_LAYOUT2:-$layout2}" ]; then
	cfg_repo="$project"
	# The people the dev bindings give `propose` on the project, so `{slug}-writers` (PF-87).
	writers="${JC_SMOKE_FORGE_WRITERS:-demo.steward}"
	folder="$base/git/$forge_org/$project/src/branch/main"
else
	cfg_repo=configuration
	folder="$base/git/$forge_org/configuration/src/branch/main/projects/$project"
fi
signed=""
: "${writers:=}"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail=0
ok() { printf '  ok    %s\n' "$1"; }
ko() { printf '  FAIL  %s\n' "$1"; fail=1; }

# The first message Keycloak or the forge shows a person, or nothing.
error_of() {
	tr '\n' ' ' < "$1" \
		| grep -o -E '(input-error|kc-error-message|kc-feedback-text|flash-error)[^>]*>([[:space:]]*<[^>]*>)*[^<]+' \
		| head -1 | sed -e 's/.*>//' -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//'
}

# The project folder lists its manifest; a login page or a 404 does not.
lists_files() { grep -q 'project\.yaml' "$1"; }

code=$(curl -sS -o "$work/anonymous" -w '%{http_code}' --max-time 20 "$folder" 2>/dev/null)
if [ "$code" = 200 ] && lists_files "$work/anonymous"; then
	ko "an anonymous visitor reads the $cfg_repo repository"
else
	ok "an anonymous visitor does not read the $cfg_repo repository ($code)"
fi

# RFC 6238 code of the seed on stdin: HMAC-SHA256 over its UTF-8 bytes, 30 s, 6 digits, as the
# realm's OTP policy and Keycloak's own check (scripts/keycloak-login-journey.py totp()).
totp_code() {
	python3 -c 'import hashlib,hmac,struct,sys,time
m=hmac.new(sys.stdin.read().encode(),struct.pack(">Q",int(time.time())//30),hashlib.sha256).digest()
o=m[-1]&15
print(str((struct.unpack(">I",m[o:o+4])[0]&0x7FFFFFFF)%10**6).zfill(6),end="")'
}

login() {
	local user="$1" password jar action page after landed code
	password=$(kubectl get secret "keycloak-user-${user//./-}" -n "$slug" -o jsonpath='{.data.password}' 2>/dev/null | base64 -d 2>/dev/null || true)
	if [ -z "$password" ]; then
		ko "$user: no password in Secret keycloak-user-${user//./-} (development profile not applied?)"
		return
	fi
	jar="$work/$user.jar" page="$work/$user.form" after="$work/$user.after"

	curl -sS -L --max-time 30 -c "$jar" -b "$jar" -o "$page" "$base/git/user/oauth2/keycloak" 2>/dev/null
	action=$(tr '\n' ' ' < "$page" | sed -n 's/.*id="kc-form-login"[^>]*action="\([^"]*\)".*/\1/p' | sed 's/&amp;/\&/g')
	if [ -z "$action" ]; then
		ko "$user: the forge's Keycloak button reaches no login form: $(error_of "$page")"
		return
	fi

	landed=$(printf '%s' "$password" | curl -sS -L --max-time 30 -c "$jar" -b "$jar" -o "$after" \
		-w '%{url_effective}' --data-urlencode "username=$user@$org" --data-urlencode "password@-" "$action" 2>/dev/null)
	# A holder of `mfa-required` (T-5120) answers Keycloak's TOTP form with the seed beside the password.
	if grep -q 'id="kc-otp-login-form"' "$after"; then
		local seed otp_action
		seed=$(kubectl get secret "keycloak-user-${user//./-}" -n "$slug" -o jsonpath='{.data.totp}' 2>/dev/null | base64 -d 2>/dev/null || true)
		if [ -z "$seed" ]; then
			ko "$user: Keycloak asks for a TOTP code and Secret keycloak-user-${user//./-} holds no totp seed"
			return
		fi
		otp_action=$(tr '\n' ' ' < "$after" | sed -n 's/.*id="kc-otp-login-form"[^>]*action="\([^"]*\)".*/\1/p' | sed 's/&amp;/\&/g')
		landed=$(printf '%s' "$seed" | totp_code | curl -sS -L --max-time 30 -c "$jar" -b "$jar" -o "$after" \
			-w '%{url_effective}' --data-urlencode "otp@-" "$otp_action" 2>/dev/null)
	fi
	case "$landed" in
		"$base/git/"*) ;;
		*) ko "$user: the login did not come back to the forge: $(error_of "$after")"; return ;;
	esac
	if [ -n "$(error_of "$after")" ] || grep -q 'id="kc-form-login"' "$after"; then
		ko "$user: the forge refused the login: $(error_of "$after")"
		return
	fi

	code=$(curl -sS -b "$jar" -o "$work/$user.folder" -w '%{http_code}' --max-time 20 "$folder" 2>/dev/null)
	if [ "$code" = 200 ] && lists_files "$work/$user.folder"; then
		ok "$user signs in to the forge through Keycloak and reads the $cfg_repo repository"
	elif [ "$code" = 404 ]; then
		ko "$user: signed in, but the $cfg_repo repository answers 404: the groups claim maps to no forge team (gitea.forge.groupTeams)"
		return
	else
		ko "$user: signed in, but the $cfg_repo repository answers $code"
		return
	fi
	# The team the `groups` claim maps onto (PF-79): the forge shows an organization's team only
	# to its members, so a 404 here is a person the map landed nowhere.
	code=$(curl -sS -b "$jar" -o /dev/null -w '%{http_code}' --max-time 20 "$base/git/org/$forge_org/teams/readers" 2>/dev/null)
	if [ "$code" = 200 ]; then
		ok "$user is in the forge team readers"
	else
		ko "$user is not in the forge team readers ($code)"
	fi
	# The same team in the Apps' own organization (T-3030): without it, a reader opens none of them.
	if [ "$apps_org" != "$forge_org" ]; then
		code=$(curl -sS -b "$jar" -o /dev/null -w '%{http_code}' --max-time 20 "$base/git/org/$apps_org/teams/readers" 2>/dev/null)
		if [ "$code" = 200 ]; then
			ok "$user is in the forge team readers of $apps_org"
		else
			ko "$user is not in the forge team readers of $apps_org ($code)"
		fi
	fi
	signed="$signed $user"
	# Every write goes through a Change (CC-41). Gitea links "Add file" for everyone; to a
	# person without write it is the "fork to propose changes" notice, to a writer the commit
	# form, so the form is what gives write access away.
	# At layout 2 a person with `propose` is in `{slug}-writers` (PF-87) and gets the form, but main
	# is protected for the Portal alone: the forge must offer them a new branch and refuse main.
	curl -sS -b "$jar" -o "$work/$user.editor" --max-time 20 "${folder/\/src\/branch\//\/_new\/}" 2>/dev/null
	if ! grep -q 'name="commit_choice"' "$work/$user.editor"; then
		ok "$user reads the $cfg_repo repository without write access"
	elif [[ " $writers " == *" $user "* ]] && grep -q 'Not allowed to commit directly to branch' "$work/$user.editor"; then
		ok "$user may propose a branch in the $cfg_repo repository, main refuses their commit"
	else
		ko "$user may commit to the $cfg_repo repository in the forge"
	fi
	# Nor a copy of it (T-1461): the forge refuses a reader's fork, which would outlive their
	# group membership. The fork page carries the form's token and the person's own id.
	local repo="${folder%%/src/branch/*}" fork_page="$work/$user.fork" token uid
	curl -sS -b "$jar" -c "$jar" -o "$fork_page" --max-time 20 "$repo/fork" 2>/dev/null
	token=$(sed -n 's/.*name="_csrf" value="\([^"]*\)".*/\1/p' "$fork_page" | head -1)
	uid=$(sed -n 's/.*name="uid"[^>]*value="\([0-9]*\)".*/\1/p' "$fork_page" | head -1)
	curl -sS -b "$jar" -c "$jar" -o /dev/null --max-time 30 -X POST "$repo/fork" \
		--data-urlencode "_csrf=$token" --data-urlencode "uid=$uid" \
		--data-urlencode "repo_name=$cfg_repo" 2>/dev/null
	code=$(curl -sS -b "$jar" -o /dev/null -w '%{http_code}' --max-time 20 "$base/git/$user/$cfg_repo" 2>/dev/null)
	if [ "$code" = 200 ]; then
		ko "$user forked the $cfg_repo repository into $base/git/$user/$cfg_repo"
	else
		ok "$user cannot fork the $cfg_repo repository ($code)"
	fi
}

for user in ${JC_SMOKE_FORGE_USERS:-demo.steward demo.viewer}; do
	login "$user"
done

# The application repositories (T-2601, PF-79, AP-78): `readers` holds every repository of the
# organization, so each one the organization page lists opens and downloads with its source for a
# person who only reads, and for nobody signed out. The list is every person's together: a
# repository one of them misses is the defect, not a reason to skip it.
apps=$(for user in $signed; do
	curl -sS -b "$work/$user.jar" --max-time 20 "$base/git/$apps_org" 2>/dev/null \
		| grep -o "href=\"/git/$apps_org/[A-Za-z0-9._-]*\"" | sed -e 's/.*\///' -e 's/"$//'
done | grep -vx "$cfg_repo" | grep -vx configuration | sort -u)
# What the project declares (projects/{project}/apps/{app}/app.yaml), read as the same people: an
# empty list beside a declared App is a reader who cannot see the repositories, not an empty forge.
declared=$(for user in $signed; do
	curl -sS -b "$work/$user.jar" --max-time 20 "$folder/apps" 2>/dev/null \
		| grep -o "href=\"${folder#"$base"}/apps/[A-Za-z0-9._-]*\"" | sed -e 's/.*\///' -e 's/"$//'
done | sort -u)
if [ -n "$signed" ] && [ -z "$apps" ]; then
	if [ -n "$declared" ]; then
		ko "$project declares Apps ($(echo $declared | tr ' ' ',')) and no reader lists a repository of $apps_org"
	else
		ok "the forge holds no application repository yet"
	fi
fi
for app in $apps; do
	repo="$base/git/$apps_org/$app"
	code=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 "$repo" 2>/dev/null)
	if [ "$code" = 404 ]; then
		ok "an anonymous visitor does not read $app (404)"
	else
		ko "an anonymous visitor reads $app ($code)"
	fi
	for user in $signed; do
		code=$(curl -sS -b "$work/$user.jar" -o "$work/$user.app" -w '%{http_code}' --max-time 20 "$repo" 2>/dev/null)
		if [ "$code" != 200 ]; then
			ko "$user: application repository $app answers $code"
			continue
		fi
		if ! grep -q "/git/$apps_org/$app/src/branch/" "$work/$user.app"; then
			ko "$user: application repository $app lists no files (empty repository?)"
			continue
		fi
		code=$(curl -sS -b "$work/$user.jar" -o "$work/$user.tgz" -w '%{http_code}' --max-time 60 "$repo/archive/main.tar.gz" 2>/dev/null)
		files=$( { tar tzf "$work/$user.tgz" 2>/dev/null || true; } | grep -vc '/$' || true)
		if [ "$code" = 200 ] && [ "${files:-0}" -gt 0 ]; then
			ok "$user reads and downloads application repository $app ($files files)"
		else
			ko "$user: downloading $app answers $code with ${files:-0} files"
		fi
	done
done
exit "$fail"
