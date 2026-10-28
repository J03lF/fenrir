#!/usr/bin/env bash
# Tests of server/bootstrap-admin.sh against a stand-in of the API gateway
# (python3, loopback, no server needed). curl and jq are wrapped so that
# every argv they are started with is recorded: neither the token nor the
# password may ever show up there, nor in the script's output.
#
#   infra/beta/tests/bootstrap-admin-test.sh
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
script=$here/server/bootstrap-admin.sh
command -v python3 >/dev/null || {
	echo "SKIP bootstrap-admin: python3 missing"
	exit 0
}
command -v jq >/dev/null || {
	echo "SKIP bootstrap-admin: jq missing"
	exit 0
}

tmp=$(mktemp -d)
pid=""
cleanup() {
	if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
	rm -rf -- "$tmp"
}
trap cleanup EXIT

TOKEN=setup_0123456789abcdef0123456789abcdef
PASSWORD='Korrekt-Pferd-Batterie-42'
fails=0
passes=0
ok() {
	passes=$((passes + 1))
	printf 'ok   %s\n' "$1"
}
bad() {
	fails=$((fails + 1))
	printf 'FAIL %s\n' "$1" >&2
}

# --- the stand-in -------------------------------------------------------------
# State in $tmp/state.json (written by the test); every request body is
# appended to $tmp/requests.log as "<path> <body>".
cat >"$tmp/stub.py" <<'PY'
import json, sys, http.server, os
state_file, log_file, port_file = sys.argv[1], sys.argv[2], sys.argv[3]
PIN, TOKEN = "424242", "setup_0123456789abcdef0123456789abcdef"
CID = "6f1c2a3b-0000-4000-8000-000000000001"

def state():
    with open(state_file) as f:
        return json.load(f)

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def send(self, status, payload):
        raw = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
    def route(self):
        p = self.path.split("/api/v1", 1)[-1]
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        with open(log_file, "a") as f:
            f.write(f"{p} {body}\n")
        return p, body
    def do_GET(self):
        p, _ = self.route()
        s = state()
        if p == "/public/site":
            return self.send(200, {"data": {"setup_required": s["empty"], "legal": {"version": "2026-10"}}})
        self.send(404, {"error": {"code": "ATH-NF-001", "message": "not found"}})
    def do_POST(self):
        p, body = self.route()
        s = state()
        req = json.loads(body) if body else {}
        if p == "/auth/register/request":
            if req.get("api_key") != TOKEN or s.get("token_refused"):
                return self.send(401, {"error": {"code": "ATH-AUTH-001", "message": "unauthorized"}})
            if req.get("accepted_legal_version") != "2026-10":
                return self.send(400, {"error": {"code": "ATH-LEGAL-001", "message": "consent"}})
            return self.send(200, {"data": {"challenge_id": CID, "expires_at": "2099-01-01T00:00:00Z"}})
        if p == f"/auth/challenge/{CID}/resend":
            return self.send(200, {"data": {"resends_remaining": 4, "retry_after_seconds": 30}})
        if p == "/auth/register/confirm":
            if req.get("challenge_id") != CID or req.get("pin") != PIN:
                return self.send(401, {"error": {"code": "AUTH-CRED-001", "message": "invalid code"}})
            return self.send(200, {"data": {"access_token": "at_secret_session", "user": {"role": s.get("role", "admin")}}})
        self.send(404, {"error": {"code": "ATH-NF-001", "message": "not found"}})

srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(port_file, "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
echo '{"empty": true}' >"$tmp/state.json"
python3 "$tmp/stub.py" "$tmp/state.json" "$tmp/requests.log" "$tmp/port" &
pid=$!
for _ in $(seq 1 50); do
	[ -s "$tmp/port" ] && break
	sleep 0.1
done
[ -s "$tmp/port" ] || {
	echo "FAIL the stand-in did not start" >&2
	exit 1
}
base="http://127.0.0.1:$(cat "$tmp/port")/gateway/services/x/api/v1"

# --- argv recorders for curl and jq -------------------------------------------
mkdir -p "$tmp/bin"
for tool in curl jq; do
	real=$(command -v "$tool")
	cat >"$tmp/bin/$tool" <<SH
#!/usr/bin/env bash
printf '%s ' "$tool" "\$@" >>"$tmp/argv.log"
printf '\n' >>"$tmp/argv.log"
exec "$real" "\$@"
SH
	chmod 755 "$tmp/bin/$tool"
done

# run NAME STDIN [state json] [token-file mode|none]: one run of the script.
run() {
	local label=$1 input=$2 st=${3:-'{"empty": true}'} tmode=${4:-600}
	printf '%s\n' "$st" >"$tmp/state.json"
	: >"$tmp/requests.log"
	: >"$tmp/argv.log"
	rm -f "$tmp/token"
	if [ "$tmode" != none ]; then
		printf '%s\n' "$TOKEN" >"$tmp/token"
		chmod "$tmode" "$tmp/token"
	fi
	set +e
	printf '%b' "$input" | PATH="$tmp/bin:$PATH" bash "$script" --local --base "$base" \
		--token-file "$tmp/token" --email admin@example.org --name "Erste Admin" >"$tmp/out" 2>&1
	rc=$?
	set -e
	if grep -qF -- "$TOKEN" "$tmp/out" "$tmp/argv.log" || grep -qF -- "$PASSWORD" "$tmp/out" "$tmp/argv.log"; then
		bad "$label: token or password in the output or an argv"
		sed 's/^/     /' "$tmp/out" >&2
	fi
	if grep -qF at_secret_session "$tmp/out"; then bad "$label: the session token was printed"; fi
}
expect() { # expect LABEL CONDITION-DESCRIPTION COMMAND...
	local label=$1
	shift
	if "$@"; then ok "$label"; else
		bad "$label"
		sed 's/^/     /' "$tmp/out" >&2
	fi
}
said() { grep -qF -- "$1" "$tmp/out"; }
sent() { grep -q -- "$1" "$tmp/requests.log"; }

# 1. An installation with an active account: refused before anything is sent.
run "active user" "j\n$PASSWORD\n$PASSWORD\n424242\n" '{"empty": false}'
expect "refuses when an active user exists" test "$rc" -ne 0
expect "  says why" said "Es gibt schon ein aktives Konto"
expect "  sends no registration" bash -c "! grep -q register '$tmp/requests.log'"

# 2. The happy path, with one wrong PIN and one resend on the way.
run "happy path" "j\n$PASSWORD\n$PASSWORD\n111111\nr\n424242\n"
expect "happy path exits 0" test "$rc" -eq 0
expect "  reports the admin" said "admin@example.org ist Plattform-Admin"
expect "  sends token, password and legal version in the body" \
	grep -qF "\"api_key\":\"$TOKEN\"" "$tmp/requests.log"
expect "  the body has the password" grep -qF "\"password\":\"$PASSWORD\"" "$tmp/requests.log"
expect "  the body accepts the legal version" grep -qF '"accepted_legal_version":"2026-10"' "$tmp/requests.log"
expect "  a wrong PIN asks again" said "Der Code stimmt nicht"
expect "  r resends" sent "^/auth/challenge/.*/resend"
expect "  the spent token file is deleted" test ! -e "$tmp/token"
expect "  curl only ever gets the body on stdin" bash -c "! grep -E '^curl .*(-d |--data |--data-raw|--json)' '$tmp/argv.log'"

# 3. Passwords that differ are asked again, nothing goes out before.
run "password retry" "j\nnicht-gleich-1234\nnicht-gleich-5678\n$PASSWORD\n$PASSWORD\n424242\n"
expect "differing passwords are asked again" test "$rc" -eq 0
expect "  and said so" said "verschieden"

# 4. No token file.
run "no token" "j\n" '{"empty": true}' none
expect "no token file: refused" test "$rc" -ne 0
expect "  tells how to get a new one" said "Kein Setup-Token"

# 5. A token file others can read.
run "loose mode" "j\n" '{"empty": true}' 644
expect "a token file readable by others: refused" test "$rc" -ne 0
expect "  and says so" said "andere können ihn lesen"
expect "  sends no registration" bash -c "! grep -q register '$tmp/requests.log'"

# 6. The server refuses the token (expired or spent).
run "token refused" "j\n$PASSWORD\n$PASSWORD\n" '{"empty": true, "token_refused": true}'
expect "a refused token: clear error" test "$rc" -ne 0
expect "  says expired or spent and how to renew" said "abgelaufen (24 h)"
expect "  the file stays" test -e "$tmp/token"

# 7. No consent: nothing is sent.
run "no consent" "n\n"
expect "without consent nothing is sent" bash -c "! grep -q register '$tmp/requests.log'"

# 8. Confirmed, but no admin (another attempt won).
run "no admin" "j\n$PASSWORD\n$PASSWORD\n424242\n" '{"empty": true, "role": "user"}'
expect "an active non-admin account is reported" test "$rc" -ne 0
expect "  with the log events to look for" said "kein Admin"

# 9. Abort at the PIN: the hint for a stuck registration.
run "abort" "j\n$PASSWORD\n$PASSWORD\nq\n"
expect "q aborts with the hint to run again" said "Starte das Skript einfach noch einmal"

# 10. bash -x must not trace the secrets.
printf '%s\n' '{"empty": true}' >"$tmp/state.json"
printf '%s\n' "$TOKEN" >"$tmp/token"
chmod 600 "$tmp/token"
set +e
printf 'j\n%s\n%s\n424242\n' "$PASSWORD" "$PASSWORD" | PATH="$tmp/bin:$PATH" bash -x "$script" --local \
	--base "$base" --token-file "$tmp/token" --email admin@example.org --name "Erste Admin" >"$tmp/out" 2>&1
set -e
if grep -qF -- "$PASSWORD" "$tmp/out" || grep -qF -- "$TOKEN" "$tmp/out"; then
	bad "bash -x prints a secret"
else ok "bash -x prints no secret"; fi

echo "bootstrap-admin: $passes ok, $fails failed"
[ "$fails" -eq 0 ]
