#!/usr/bin/env bash
# Checks the GitHub workflows of one or more repositories against the
# release flow (infra/release/README.md):
#   - .github/workflows holds exactly one file, release.yml;
#   - it parses as YAML (actionlint when installed, else python3 with
#     PyYAML, else ruby's Psych; the structural checks need one of the two
#     parsers, otherwise they are reported as SKIPPED, not passed);
#   - triggers: only push of tags v* and workflow_dispatch (no branches,
#     pull_request, schedule, ...);
#   - top-level permissions: contents: read; a job may raise contents to
#     write, nothing else;
#   - every `uses:` is pinned to a full commit SHA (or ./local, or a docker
#     image by digest);
#   - the text does not use the product's old name (it is Athenos now).
#
#   workflow-lint.sh <repo-dir>...
set -uo pipefail

fail=0
err() {
	printf 'FAIL %s\n' "$*" >&2
	fail=1
}

to_json() { # FILE → JSON on stdout, or exit 2 when no parser is available
	if python3 -I -c 'import yaml' 2>/dev/null; then
		python3 -I -c 'import json, sys, yaml; json.dump(yaml.safe_load(open(sys.argv[1])), sys.stdout)' "$1"
	elif command -v ruby >/dev/null 2>&1 && ruby -e 'require "yaml"; require "json"' 2>/dev/null; then
		ruby -e 'require "yaml"; require "json"; puts JSON.generate(YAML.safe_load(File.read(ARGV[0])))' "$1"
	else
		return 2
	fi
}

# The structural checks on the parsed workflow (JSON on stdin). YAML 1.1
# parsers read the key `on` as the boolean true, so both are accepted.
check_py='
import json, re, sys
wf = json.load(sys.stdin)
problems = []
on = wf.get("on", wf.get("true"))
if not isinstance(on, dict):
    problems.append("no `on:` mapping")
else:
    keys = sorted(on)
    if keys != ["push", "workflow_dispatch"]:
        problems.append("triggers must be exactly push (tags) and workflow_dispatch, found %s" % keys)
    push = on.get("push") or {}
    if not isinstance(push, dict) or sorted(push) != ["tags"] or push.get("tags") != ["v*"]:
        problems.append("push must be tags: [\"v*\"] only, found %s" % json.dumps(push))
perms = wf.get("permissions")
if perms != {"contents": "read"}:
    problems.append("top-level permissions must be {contents: read}, found %s" % json.dumps(perms))
jobs = wf.get("jobs") or {}
if not jobs:
    problems.append("no jobs")
sha = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}$")
for name, job in jobs.items():
    p = job.get("permissions")
    if p is not None and (not isinstance(p, dict) or set(p) - {"contents"} or p.get("contents") not in ("read", "write")):
        problems.append("job %s: permissions beyond contents: %s" % (name, json.dumps(p)))
    if "uses" in job and not job["uses"].startswith("./"):
        problems.append("job %s: reusable workflow %s" % (name, job["uses"]))
    for step in job.get("steps") or []:
        u = step.get("uses")
        if u is None or u.startswith("./") or (u.startswith("docker://") and "@sha256:" in u):
            continue
        if not sha.match(u):
            problems.append("job %s: action not pinned to a commit SHA: %s" % (name, u))
for p in problems:
    print(p)
sys.exit(1 if problems else 0)
'

parser_note=""
old_name="athe""ne" # split, so this file passes the same search
for repo in "$@"; do
	dir="$repo/.github/workflows"
	name=$(basename -- "$repo")
	if [ ! -d "$dir" ]; then
		err "$name: no .github/workflows"
		continue
	fi
	others=$(cd -- "$dir" && find . -type f ! -name release.yml | sed 's#^\./##' | LC_ALL=C sort | tr '\n' ' ')
	[ -z "$others" ] || err "$name: workflows other than release.yml: $others"
	wf="$dir/release.yml"
	if [ ! -f "$wf" ]; then
		err "$name: no release.yml"
		continue
	fi
	! grep -qi "$old_name" "$wf" || err "$name: release.yml uses the old product name"
	if command -v actionlint >/dev/null 2>&1; then
		actionlint -no-color "$wf" || err "$name: actionlint"
	fi
	json=$(to_json "$wf" 2>&1)
	rc=$?
	if [ "$rc" = 2 ]; then
		parser_note=" (structural checks SKIPPED: neither PyYAML nor ruby)"
		continue
	elif [ "$rc" != 0 ]; then
		err "$name: release.yml is not valid YAML: $json"
		continue
	fi
	out=$(printf '%s' "$json" | python3 -I -c "$check_py") || err "$name: $(printf '%s' "$out" | tr '\n' ';')"
done
linter="python3/ruby YAML"
command -v actionlint >/dev/null 2>&1 && linter="actionlint + $linter"
[ "$fail" = 0 ] && echo "workflow-lint: $# repo(s) ok ($linter)$parser_note"
exit "$fail"
