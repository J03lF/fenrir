#!/usr/bin/env bash
# The first Athenos administrator, from the console only (there is no web
# page for it). One script for the beta server and for a local Fenrir:
#
#   Server:  sudo /usr/local/lib/athenos/bootstrap-admin.sh --email du@example.org --name "Dein Name"
#   Mac:     infra/beta/server/bootstrap-admin.sh --local --email du@example.org --name "Dein Name"
#
# How it works (SPEC §6, W9 A11): while the installation has no active
# account, the core writes a one-time setup token (24 h, only its hash is
# stored) to <runtime>/secrets/athenos-setup-token, owner-only. Every
# registration then needs it as `api_key`; the confirmation with the PIN
# consumes it and makes the account the platform admin. This script:
#   1. asks GET /public/site whether the installation is still empty
#      (`setup_required`) and refuses otherwise;
#   2. reads the token from the 0600 file itself;
#   3. asks you to accept the current legal version, then the password twice
#      without echo;
#   4. registers through the normal API (the same checks as any sign-up);
#   5. asks for the PIN (mail; locally the PIN sink), lets you resend it,
#      confirms, checks that the account is admin and deletes the spent
#      token file.
#
# Secrets never touch argv, the environment or a file of ours: token and
# password reach jq through `--rawfile` on a pipe fed by a shell builtin,
# and the request body reaches curl on stdin (`--data-binary @-`). Nothing
# prints them; xtrace is switched off even under `bash -x`.
#
# Interrupted or stuck in "pending"? Run it again with the same address:
# a new registration replaces the open one, and the token is only spent by
# a confirmation.
#
# Options:
#   --email ADDR        the admin's address (the PIN goes there)
#   --name NAME         display name
#   --local             a local Fenrir (default when /var/lib/athenos is absent)
#   --server            the beta server (default when /var/lib/athenos exists)
#   --token-file PATH   where the token is (default per profile, or
#                       ATHENOS_SETUP_TOKEN_FILE)
#   --base URL          the API gateway (default: the edge on 127.0.0.1:41002,
#                       or ATHENOS_BASE)
set +x
set -euo pipefail
umask 077

usage() {
	sed -n '/^# Options:/,/^set +x/p' "$0" | sed '$d; s/^# \{0,1\}//' >&2
	printf '\nBeispiel: %s --email du@example.org --name "Dein Name"\n' "${0##*/}" >&2
	exit 2
}
fail() {
	printf 'Fehler: %s\n' "$1" >&2
	shift
	local line
	for line in "$@"; do printf '  %s\n' "$line" >&2; done
	exit 1
}
say() { printf '%s\n' "$*"; }

email=""
name=""
profile=""
token_file=${ATHENOS_SETUP_TOKEN_FILE:-}
BASE=${ATHENOS_BASE:-http://127.0.0.1:41002/gateway/services/module%3Aathenos-api%3A%3Aapi-gateway/api/v1}
while [ $# -gt 0 ]; do
	case $1 in
	--email) email=${2:-} && shift ;;
	--name) name=${2:-} && shift ;;
	--local) profile=local ;;
	--server) profile=server ;;
	--token-file) token_file=${2:-} && shift ;;
	--base) BASE=${2:-} && shift ;;
	-h | --help) usage ;;
	*) printf 'Unbekannte Option: %s\n\n' "$1" >&2 && usage ;;
	esac
	shift
done
[ -n "$email" ] && [ -n "$name" ] || usage
case $email in *@*.*) ;; *) fail "\"$email\" ist keine E-Mail-Adresse." ;; esac

if [ -z "$profile" ]; then
	if [ -d /var/lib/athenos ]; then profile=server; else profile=local; fi
fi
if [ -z "$token_file" ]; then
	if [ "$profile" = server ]; then
		token_file=/var/lib/athenos/modules-runtime/secrets/athenos-setup-token
	else
		token_file=$HOME/Documents/personal/modules/runtime/secrets/athenos-setup-token
	fi
fi
if [ "$profile" = server ]; then
	login_url=https://app.athenos.dev/auth/login
	restart_hint="sudo systemctl restart athenos"
	log_hint="sudo journalctl -u athenos | grep athenos.setup_token"
	pin_hint="Der Code kommt per Mail an $email."
else
	login_url=http://localhost:41002/auth/login
	restart_hint="Fenrir neu starten (abends), dann schreibt der Core einen neuen"
	log_hint="im Core-Log nach athenos.setup_token suchen"
	pin_hint="Lokal steht der Code in der letzten Zeile des PIN-Sinks (AUTH_DEV_PIN_SINK), sonst in der Mail an $email."
fi

command -v curl >/dev/null || fail "curl fehlt."
command -v jq >/dev/null || fail "jq fehlt (Server: sudo apt install jq; Mac: brew install jq)."

# --- HTTP ------------------------------------------------------------------
# api METHOD PATH [body on stdin] -> sets $status and $body. A body only
# ever comes from stdin, so it never shows up in a process list.
status=""
body=""
api() {
	local method=$1 path=$2 out
	if [ "$method" = GET ]; then
		out=$(curl -sS -m 20 -w '\n%{http_code}' "$BASE$path" 2>&1) || out=$(printf '%s\n000' "$out")
	else
		out=$(curl -sS -m 30 -X "$method" -H 'content-type: application/json' \
			--data-binary @- -w '\n%{http_code}' "$BASE$path" 2>&1) || out=$(printf '%s\n000' "$out")
	fi
	status=${out##*$'\n'}
	body=${out%$'\n'*}
}
field() { printf '%s' "$body" | jq -r "$1 // empty" 2>/dev/null || true; }
error_code() { field '.error.details.upstream_code // .error.code // .code'; }
error_message() { field '.error.message // .message'; }
unreachable() {
	fail "Athenos antwortet nicht unter $BASE." \
		"Läuft der Host? Server: sudo systemctl status athenos; lokal: läuft Fenrir?" \
		"curl: $(printf '%s' "$body" | tail -n 1)"
}

# --- 1. Is the installation still empty? -----------------------------------
api GET /public/site
[ "$status" = 000 ] && unreachable
[ "$status" = 200 ] || fail "GET /public/site antwortet $status: $(error_message)"
required=$(field '.data.setup_required | tostring')
legal=$(field '.data.legal.version')
case $required in
true) ;;
false)
	fail "Es gibt schon ein aktives Konto. Der erste Admin ist eingerichtet; dieses Skript tut nichts mehr." \
		"Weitere Admins vergibst du in der App unter Admin → Benutzer."
	;;
*) fail "Der Server sagt nicht, ob er leer ist (setup_required fehlt). Ist das Athenos 0.10 oder neuer?" ;;
esac
[ -n "$legal" ] || fail "Der Server nennt keine Version der Rechtstexte (data.legal.version)."

# --- 2. The token file ------------------------------------------------------
if [ ! -e "$token_file" ]; then
	fail "Kein Setup-Token unter $token_file." \
		"Der Core schreibt ihn beim Start, solange niemand aktiv ist. Neuer Token: $restart_hint." \
		"Was der Core dazu sagt: $log_hint (minted, outstanding oder failed)."
fi
if [ ! -r "$token_file" ]; then
	if [ "$profile" = server ]; then
		fail "$token_file ist für $(id -un) nicht lesbar." \
			"Starte das Skript mit sudo (oder als Dienstbenutzer: sudo -u athenos …)."
	fi
	fail "$token_file ist für $(id -un) nicht lesbar."
fi
mode=$(stat -c %a "$token_file" 2>/dev/null || stat -f %Lp "$token_file")
case $mode in
600 | 400) ;;
*)
	fail "$token_file hat die Rechte $mode, andere können ihn lesen. Behandle ihn als verbraucht:" \
		"Datei löschen, dann $restart_hint (der Core schreibt einen neuen mit 0600)."
	;;
esac
if ! tr -d ' \t\r\n' <"$token_file" | grep -Eq '^setup_[0-9a-f]{32}$'; then
	fail "$token_file enthält keinen Setup-Token (erwartet: setup_ und 32 Hex-Zeichen)." \
		"Datei löschen, dann $restart_hint."
fi

# --- 3. Consent and password ------------------------------------------------
say "Erster Admin für Athenos ($profile): $name <$email>"
say "Mit der Registrierung nimmst du die Nutzungsbedingungen und die Datenschutzerklärung"
say "in der Version $legal an (in der App unter /nutzungsbedingungen und /datenschutz)."
read -r -p "Annehmen? [j/N] " yes || yes=""
case $yes in j | J | ja | Ja | y | Y | yes) ;; *) fail "Abgebrochen, nichts wurde gesendet." ;; esac

pw=""
for attempt in 1 2 3; do
	read -r -s -p "Passwort für $email (mindestens 12 Zeichen): " pw || pw=""
	echo
	read -r -s -p "Noch einmal: " pw2 || pw2=""
	echo
	if [ "$pw" != "$pw2" ]; then
		say "Die beiden Eingaben sind verschieden."
	elif [ ${#pw} -lt 12 ]; then
		say "Zu kurz: mindestens 12 Zeichen."
	else
		break
	fi
	pw=""
	[ "$attempt" = 3 ] && fail "Drei Versuche ohne gültiges Passwort, nichts wurde gesendet."
done
unset pw2

# --- 4. Register --------------------------------------------------------------
# The trailing newline the core writes and any whitespace go; the value
# itself was checked above.
# api runs in this shell (a pipe would put it in a subshell and lose
# $status), so the body comes in through a process substitution.
api POST /auth/register/request < <(jq -cn --arg e "$email" --arg n "$name" --arg l "$legal" \
	--rawfile p <(printf '%s' "$pw") --rawfile k "$token_file" \
	'{email: $e, display_name: $n, password: $p, accepted_legal_version: $l,
	  api_key: ($k | gsub("\\s"; ""))}')
unset pw
case $status in
200 | 201) ;;
000) unreachable ;;
401)
	fail "Der Server nimmt den Setup-Token nicht an: abgelaufen (24 h), schon verbraucht oder nicht der aktuelle." \
		"Neuer Token: $restart_hint. Danach das Skript noch einmal starten." \
		"Was der Core dazu sagt: $log_hint."
	;;
400)
	if [ "$(error_code)" = ATH-LEGAL-001 ]; then
		fail "Die Rechtstexte haben gerade eine neue Version bekommen. Starte das Skript noch einmal."
	fi
	fail "Die Registrierung wurde abgelehnt: $(error_message) ($(error_code))." \
		"Prüfe Adresse, Name und Passwortregeln (Produktion: lang und nicht zu einfach)."
	;;
403) fail "Die Registrierung ist verboten: $(error_message) ($(error_code)). Erlaubte Mail-Domains in den Einstellungen?" ;;
429) fail "Zu viele Versuche in kurzer Zeit: $(error_message). Warte ein paar Minuten und starte dann neu." ;;
*) fail "Die Registrierung schlug fehl (HTTP $status): $(error_message) ($(error_code))." ;;
esac
cid=$(field '.data.challenge_id // .challenge_id')
[ -n "$cid" ] || fail "Die Antwort enthält keine Challenge."
case $cid in *[!0-9A-Za-z-]*) fail "Unerwartete Challenge-ID in der Antwort." ;; esac

# --- 5. PIN -------------------------------------------------------------------
stuck_hint="Hängt es? Starte das Skript einfach noch einmal mit derselben Adresse: Das ersetzt die offene Registrierung, und der Token gilt weiter, bis eine Bestätigung klappt."
say "Ein Bestätigungscode ist unterwegs. $pin_hint"
say "Eingabe: den Code; r = neuen Code senden; q = abbrechen."
while :; do
	read -r -p "Code: " pin || fail "Eingabe beendet." "$stuck_hint"
	pin=$(printf '%s' "$pin" | tr -d ' \t\r')
	case $pin in
	q | Q) fail "Abgebrochen. Das Konto bleibt unbestätigt, der Token bleibt gültig." "$stuck_hint" ;;
	r | R)
		api POST "/auth/challenge/$cid/resend" </dev/null
		case $status in
		200) say "Ein neuer Code ist unterwegs; der alte gilt nicht mehr. Noch $(field '.data.resends_remaining | tostring') Mal möglich." ;;
		429)
			if [ "$(field '.error.details.reason')" = exhausted ]; then
				fail "Keine weiteren Codes für diese Registrierung." "$stuck_hint"
			fi
			say "Noch zu früh für einen neuen Code. Warte $(field '.error.details.retry_after_seconds | tostring') Sekunden."
			;;
		000) unreachable ;;
		*) fail "Kein neuer Code (HTTP $status): $(error_message)." "$stuck_hint" ;;
		esac
		continue
		;;
	esac
	case $pin in
	'' | *[!0-9]*)
		say "Der Code besteht nur aus Ziffern."
		continue
		;;
	esac
	api POST /auth/register/confirm < <(jq -cn --arg c "$cid" \
		--rawfile p <(printf '%s' "$pin") '{challenge_id: $c, pin: $p}')
	case $status in
	200 | 201) break ;;
	000) unreachable ;;
	401)
		say "Der Code stimmt nicht oder ist abgelaufen (er gilt ein paar Minuten). Nochmal eingeben oder r für einen neuen."
		say "Nach fünf falschen Codes ist das Konto eine Weile gesperrt."
		;;
	403)
		fail "Zu viele falsche Codes: das Konto ist gesperrt." \
			"Warte eine halbe Stunde und starte das Skript dann noch einmal (neuer Code, der Token gilt weiter)."
		;;
	404 | 410) fail "Diese Registrierung ist abgelaufen." "$stuck_hint" ;;
	429) say "Zu viele Versuche. Warte eine Minute und gib den Code dann noch einmal ein." ;;
	*) fail "Die Bestätigung schlug fehl (HTTP $status): $(error_message) ($(error_code))." "$stuck_hint" ;;
	esac
done

# The answer carries the new session; only the role is kept.
role=$(field '.data.user.role')
body=""
if [ "$role" != admin ]; then
	fail "Das Konto $email ist aktiv, aber kein Admin (Rolle: ${role:-unbekannt})." \
		"Ein anderer Versuch hat den Token zuerst verbraucht, oder die Beförderung schlug fehl." \
		"Was der Core dazu sagt: $log_hint (consumed, already_used, promote_failed)."
fi
# The token is spent; the file would only confuse now.
rm -f -- "$token_file" 2>/dev/null || say "Hinweis: $token_file liess sich nicht löschen; er ist verbraucht, lösche ihn von Hand."
say "Fertig: $email ist Plattform-Admin. Melde dich an: $login_url"
