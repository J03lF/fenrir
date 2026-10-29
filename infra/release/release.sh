#!/usr/bin/env bash
# Release Fenrir/Athenos modules from the Mac: version, commit, tag, push.
# The pushed tag starts the repository's only pipeline,
# .github/workflows/release.yml, which runs the gates and attaches the
# bundles and SHA256SUMS to the GitHub release.
#
#   release.sh athenos athenos-web        each named module: next build number
#   release.sh athenos --version 1.3.0    new version, the build restarts at 1
#   release.sh --list                     current version/build of every module
#   release.sh --dry-run ...              show the plan, change nothing
#
# Options:
#   --version X.Y.Z   new core version for every named module (→ X.Y.Z-1);
#                     refused unless it is greater than the current core
#   --no-gates        skip the quick local gates (fmt/clippy or lint)
#   --dry-run, -n     preflight and summary only: no write, commit, tag or push
#   --yes, -y         do not ask before pushing (for scripts; the summary is
#                     printed anyway)
#   --list            version, next build and branch of every module
#
# Versions are X.Y.Z-N: N is a per-module build number that counts up with
# every release. The current version is the repository's newest tag
# vX.Y.Z-N, ordered by the core version and then N (numerically); a legacy
# tag vX.Y.Z counts as build 0. Other tags are ignored.
#
# Per module (in the order of modules.conf, which is the dependency order):
#   1. the repository is clean (tracked files), on its branch and not
#      behind origin; the new tag exists neither here nor on origin;
#   2. the version is written into the files modules.conf names;
#   3. the module's quick gates run (unless --no-gates);
#   4. commit "chore(release): vX.Y.Z-N" of exactly those files, as
#      $RELEASE_AUTHOR_NAME <$RELEASE_AUTHOR_EMAIL>;
#   5. annotated tag vX.Y.Z-N.
# Then one summary table and one question; only a yes pushes branch and tag
# of every repository (git push --atomic, in dependency order). Anything
# else, an error or Ctrl-C before the push removes the local release commits
# and tags again.
#
# Environment:
#   RELEASE_CONFIG        the module table (default: modules.conf next to this script)
#   RELEASE_ROOT          overrides the `root` line of the module table
#   RELEASE_AUTHOR_NAME   default Joel Furter
#   RELEASE_AUTHOR_EMAIL  default joel.furter@gmail.com
#
# Works with the bash 3.2 of macOS (no associative arrays).
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
config=${RELEASE_CONFIG:-$here/modules.conf}
author_name=${RELEASE_AUTHOR_NAME:-Joel Furter}
author_email=${RELEASE_AUTHOR_EMAIL:-joel.furter@gmail.com}

die() {
	printf 'release.sh: %s\n' "$*" >&2
	exit 1
}
warn() { printf 'warning: %s\n' "$*" >&2; }
info() { printf '%s\n' "$*"; }

usage() {
	sed -n '2,/^# Works with/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
}

# --- arguments ----------------------------------------------------------------
names=()
version_arg=""
list=0
dry_run=0
run_gates=1
assume_yes=0
while [ $# -gt 0 ]; do
	case $1 in
	--version)
		[ $# -ge 2 ] || die "--version needs X.Y.Z"
		version_arg=$2
		shift 2
		;;
	--version=*)
		version_arg=${1#--version=}
		shift
		;;
	--list) list=1 && shift ;;
	--dry-run | -n) dry_run=1 && shift ;;
	--no-gates) run_gates=0 && shift ;;
	--yes | -y) assume_yes=1 && shift ;;
	-h | --help)
		usage
		exit 0
		;;
	-*) die "unknown option $1 (see --help)" ;;
	*)
		names+=("$1")
		shift
		;;
	esac
done
if [ -n "$version_arg" ] && ! [[ $version_arg =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
	die "--version wants X.Y.Z (the build number is set by release.sh): $version_arg"
fi

# --- module table -------------------------------------------------------------
# Parallel arrays (bash 3.2): M_* per module, S_* per sibling, P_* per pin.
M_NAME=() M_PATH=() M_BRANCH=() M_WRITERS=() M_PLATFORM=() M_GATE=()
S_NAME=() S_PATH=()
P_VAR=() P_REPO=()
root=""

[ -f "$config" ] || die "module table not found: $config"
config_dir=$(cd -- "$(dirname -- "$config")" && pwd)
lineno=0
while IFS= read -r line || [ -n "$line" ]; do
	lineno=$((lineno + 1))
	line=${line%%#*}
	# shellcheck disable=SC2086 # word splitting is the parser
	set -- $line
	[ $# -gt 0 ] || continue
	kind=$1
	shift
	case $kind in
	root)
		[ $# -eq 1 ] || die "$config:$lineno: root <dir>"
		root=$1
		;;
	module)
		[ $# -ge 6 ] || die "$config:$lineno: module <name> <path> <branch> <writers> <platform> <gate...>"
		M_NAME+=("$1") M_PATH+=("$2") M_BRANCH+=("$3") M_WRITERS+=("$4") M_PLATFORM+=("$5")
		shift 5
		M_GATE+=("$*")
		;;
	sibling)
		[ $# -eq 2 ] || die "$config:$lineno: sibling <name> <path>"
		S_NAME+=("$1") S_PATH+=("$2")
		;;
	pin)
		[ $# -eq 2 ] || die "$config:$lineno: pin <VARIABLE> <repo name>"
		P_VAR+=("$1") P_REPO+=("$2")
		;;
	*) die "$config:$lineno: unknown line '$kind'" ;;
	esac
done <"$config"
set --
[ ${#M_NAME[@]} -gt 0 ] || die "$config lists no module"
for w in $(printf '%s\n' "${M_WRITERS[@]}" | tr ',' '\n'); do
	case $w in
	- | cargo | fenrir-dev | npm | npm=?* | openapi=?* | ts-const=?*) ;;
	*) die "$config: unknown version writer '$w'" ;;
	esac
done
root=${RELEASE_ROOT:-${root:-$config_dir}}
case $root in /*) ;; *) root="$config_dir/$root" ;; esac

abs_path() { # path relative to root, or absolute
	case $1 in /*) printf '%s\n' "$1" ;; *) printf '%s/%s\n' "$root" "$1" ;; esac
}

module_index() { # name → index, also "contracts" for athenos-contracts
	local i
	for i in "${!M_NAME[@]}"; do
		[ "${M_NAME[$i]}" = "$1" ] && {
			echo "$i"
			return 0
		}
	done
	for i in "${!M_NAME[@]}"; do
		[ "${M_NAME[$i]}" = "athenos-$1" ] && {
			echo "$i"
			return 0
		}
	done
	return 1
}

repo_dir_of() { # repo name (module or sibling) → directory, or nothing
	local i
	for i in "${!M_NAME[@]}"; do
		[ "${M_NAME[$i]}" = "$1" ] && {
			abs_path "${M_PATH[$i]}"
			return 0
		}
	done
	for i in ${S_NAME[@]+"${!S_NAME[@]}"}; do
		[ "${S_NAME[$i]}" = "$1" ] && {
			abs_path "${S_PATH[$i]}"
			return 0
		}
	done
	return 1
}

# --- versions -----------------------------------------------------------------
# newest_tag DIR → "MAJ MIN PAT BUILD TAG" of the newest vX.Y.Z[-N], or nothing.
newest_tag() {
	git -C "$1" tag -l 'v*' | while IFS= read -r t; do
		if [[ $t =~ ^v([0-9]+)\.([0-9]+)\.([0-9]+)(-([0-9]+))?$ ]]; then
			printf '%d %d %d %d %s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}" "${BASH_REMATCH[5]:-0}" "$t"
		fi
	done | sort -k1,1n -k2,2n -k3,3n -k4,4n | tail -n 1
}

# core_cmp A B (X.Y.Z each) → prints -1, 0 or 1
core_cmp() {
	local a1 a2 a3 b1 b2 b3
	IFS=. read -r a1 a2 a3 <<<"$1"
	IFS=. read -r b1 b2 b3 <<<"$2"
	for pair in "$a1 $b1" "$a2 $b2" "$a3 $b3"; do
		set -- $pair
		if [ "$1" -lt "$2" ]; then
			echo -1
			return
		elif [ "$1" -gt "$2" ]; then
			echo 1
			return
		fi
	done
	echo 0
}

# --- version writers ----------------------------------------------------------
# Each writer edits FILE in place and exits 3 when it found no version to
# replace. They run on a copy first (dry run and "which files change").
#   cargo           Cargo.toml [package] version + its Cargo.lock entry
#   fenrir-dev      .fenrir-dev.toml [module] version
#   openapi=PATH    the "version" of the "info" object of an OpenAPI JSON
#   npm             package.json "version" + package-lock.json (root and packages."")
#   npm=PATH        another package.json
#   ts-const=PATH   VERSION: '...' in a TypeScript constants file
#   -               nothing (the version lives in the tags only)

toml_section_version() { # SECTION FILE: first version = "..." in [SECTION]
	NEWV=$new_version SECTION=$1 perl -i -ne '
		if (/^\s*\[/) { $in = (/^\s*\[\Q$ENV{SECTION}\E\]\s*$/) ? 1 : 0 }
		if ($in && !$done && s/^(version\s*=\s*")[^"]*(")/$1$ENV{NEWV}$2/) { $done = 1 }
		print;
		END { exit($done ? 0 : 3) }' "$2"
}
cargo_lock_version() { # CRATE FILE
	NEWV=$new_version CRATE=$1 perl -0 -i -pe '
		$done = s/(\[\[package\]\]\nname = "\Q$ENV{CRATE}\E"\nversion = ")[^"]*(")/$1$ENV{NEWV}$2/;
		END { exit($done ? 0 : 3) }' "$2"
}
json_top_version() { # FILE: the first "version" at two spaces of indentation
	NEWV=$new_version perl -i -ne '
		if (!$done && s/^(  "version"\s*:\s*")[^"]*(")/$1$ENV{NEWV}$2/) { $done = 1 }
		print;
		END { exit($done ? 0 : 3) }' "$1"
}
npm_lock_version() { # FILE: root "version" and packages."" version
	json_top_version "$1" || return
	NEWV=$new_version perl -0 -i -pe '
		$done = s/("packages"\s*:\s*\{\s*""\s*:\s*\{[^{}]*?"version"\s*:\s*")[^"]*(")/$1$ENV{NEWV}$2/s;
		END { exit($done ? 0 : 3) }' "$1"
}
openapi_version() { # FILE
	NEWV=$new_version perl -0 -i -pe '
		$done = s/("info"\s*:\s*\{[^{}]*?"version"\s*:\s*")[^"]*(")/$1$ENV{NEWV}$2/s;
		END { exit($done ? 0 : 3) }' "$1"
}
ts_const_version() { # FILE
	NEWV=$new_version perl -i -ne '
		if (!$done && s/^(\s*VERSION:\s*\x27)[^\x27]*(\x27)/$1$ENV{NEWV}$2/) { $done = 1 }
		print;
		END { exit($done ? 0 : 3) }' "$1"
}
crate_name() { # Cargo.toml → [package] name
	perl -ne 'if (/^\s*\[/) { $in = /^\s*\[package\]\s*$/ } if ($in && /^name\s*=\s*"([^"]+)"/) { print "$1\n"; exit }' "$1"
}

# writer_files WRITERS → "relative-file<TAB>action" per line
writer_files() {
	local w IFS=,
	for w in $1; do
		case $w in
		-) ;;
		cargo) printf 'Cargo.toml\tcargo-toml\nCargo.lock\tcargo-lock\n' ;;
		fenrir-dev) printf '.fenrir-dev.toml\tfenrir-dev\n' ;;
		npm) printf 'package.json\tjson\npackage-lock.json\tnpm-lock\n' ;;
		npm=*) printf '%s\tjson\n' "${w#npm=}" ;;
		openapi=*) printf '%s\topenapi\n' "${w#openapi=}" ;;
		ts-const=*) printf '%s\tts-const\n' "${w#ts-const=}" ;;
		*) die "unknown version writer '$w' in $config" ;;
		esac
	done
}

# apply_writer DIR REL ACTION TARGET: write new_version into TARGET (a copy
# of DIR/REL or the file itself). Fails with a message when nothing matched.
apply_writer() {
	local dir=$1 rel=$2 action=$3 target=$4 rc=0
	case $action in
	cargo-toml) toml_section_version package "$target" || rc=$? ;;
	cargo-lock)
		local crate
		crate=$(crate_name "$dir/Cargo.toml")
		[ -n "$crate" ] || die "$dir/Cargo.toml: no [package] name"
		cargo_lock_version "$crate" "$target" || rc=$?
		;;
	fenrir-dev) toml_section_version module "$target" || rc=$? ;;
	json) json_top_version "$target" || rc=$? ;;
	npm-lock) npm_lock_version "$target" || rc=$? ;;
	openapi) openapi_version "$target" || rc=$? ;;
	ts-const) ts_const_version "$target" || rc=$? ;;
	esac
	[ "$rc" = 0 ] || die "$dir/$rel: found no version to replace ($action)"
}

# changed_files DIR WRITERS → the files (relative) the new version changes,
# computed on copies; nothing is written.
changed_files() {
	local dir=$1 rel action copy
	while IFS="$(printf '\t')" read -r rel action; do
		[ -n "$rel" ] || continue
		[ -f "$dir/$rel" ] || die "$dir/$rel does not exist (named by $config)"
		copy="$work/copy"
		cp -- "$dir/$rel" "$copy"
		apply_writer "$dir" "$rel" "$action" "$copy"
		cmp -s -- "$dir/$rel" "$copy" || printf '%s\n' "$rel"
	done < <(writer_files "$2")
}

# write_files DIR WRITERS: write the new version in place.
write_files() {
	local dir=$1 rel action copy
	while IFS="$(printf '\t')" read -r rel action; do
		[ -n "$rel" ] || continue
		copy="$work/copy"
		cp -- "$dir/$rel" "$copy"
		apply_writer "$dir" "$rel" "$action" "$copy"
		cmp -s -- "$dir/$rel" "$copy" || cat -- "$copy" >"$dir/$rel"
	done < <(writer_files "$2")
}

# file_version DIR WRITERS → the version the first writer's file holds, or -
file_version() {
	local w=${2%%,*} v=""
	case $w in
	cargo) v=$(perl -ne 'if (/^\s*\[/) { $in = /^\s*\[package\]\s*$/ } if ($in && /^version\s*=\s*"([^"]+)"/) { print "$1"; exit }' "$1/Cargo.toml" 2>/dev/null) ;;
	fenrir-dev) v=$(perl -ne 'if (/^\s*\[/) { $in = /^\s*\[module\]\s*$/ } if ($in && /^version\s*=\s*"([^"]+)"/) { print "$1"; exit }' "$1/.fenrir-dev.toml" 2>/dev/null) ;;
	npm) v=$(perl -ne 'if (/^  "version"\s*:\s*"([^"]+)"/) { print "$1"; exit }' "$1/package.json" 2>/dev/null) ;;
	esac
	printf '%s\n' "${v:--}"
}

# --- GitHub --------------------------------------------------------------------
github_slug() { # DIR → owner/repo of origin, or nothing
	local url
	url=$(git -C "$1" remote get-url origin 2>/dev/null) || return 0
	if [[ $url =~ github\.com[:/]([^/]+)/([^/]+)$ ]]; then
		printf '%s/%s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]%.git}"
	fi
}
deploy_var() { printf 'TAG_%s' "$(printf '%s' "$1" | tr 'a-z-' 'A-Z_')"; }

work=$(mktemp -d "${TMPDIR:-/tmp}/release.XXXXXX")
trap 'rm -rf -- "$work"' EXIT

# --- list ---------------------------------------------------------------------
if [ "$list" = 1 ]; then
	[ ${#names[@]} -eq 0 ] || die "--list takes no module names"
	printf '%-22s %-14s %-14s %-12s %-8s %s\n' module tag next files branch "on (HEAD)"
	for i in "${!M_NAME[@]}"; do
		dir=$(abs_path "${M_PATH[$i]}")
		if [ ! -d "$dir/.git" ] && [ ! -f "$dir/.git" ]; then
			printf '%-22s %s\n' "${M_NAME[$i]}" "(no repository at $dir)"
			continue
		fi
		git -C "$dir" fetch --quiet --tags origin 2>/dev/null || true
		newest=$(newest_tag "$dir")
		if [ -n "$newest" ]; then
			set -- $newest
			tag=$5 next="v$1.$2.$3-$(($4 + 1))"
		else
			tag="-" next="(--version)"
		fi
		set --
		on="$(git -C "$dir" branch --show-current 2>/dev/null || true) ($(git -C "$dir" rev-parse --short HEAD))"
		printf '%-22s %-14s %-14s %-12s %-8s %s\n' "${M_NAME[$i]}" "$tag" "$next" \
			"$(file_version "$dir" "${M_WRITERS[$i]}")" "${M_BRANCH[$i]}" "$on"
	done
	rm -rf -- "$work"
	exit 0
fi

[ ${#names[@]} -gt 0 ] || {
	usage >&2
	exit 2
}

# --- plan (preflight, nothing is written) --------------------------------------
# Selected modules in table order: R_* per release.
R_IDX=() R_DIR=() R_OLD=() R_NEW=() R_FILES=() R_AHEAD=() R_HEAD=()
for i in "${!M_NAME[@]}"; do
	for n in "${names[@]}"; do
		j=$(module_index "$n") || die "unknown module '$n' (known: ${M_NAME[*]})"
		if [ "$j" = "$i" ]; then
			R_IDX+=("$i")
			break
		fi
	done
done
for n in "${names[@]}"; do module_index "$n" >/dev/null || die "unknown module '$n' (known: ${M_NAME[*]})"; done

problems=()
for k in "${!R_IDX[@]}"; do
	i=${R_IDX[$k]}
	name=${M_NAME[$i]} branch=${M_BRANCH[$i]}
	dir=$(abs_path "${M_PATH[$i]}")
	R_DIR+=("$dir")
	if ! git -C "$dir" rev-parse --git-dir >/dev/null 2>&1; then
		die "$name: no git repository at $dir"
	fi
	current=$(git -C "$dir" branch --show-current)
	[ "$current" = "$branch" ] || problems+=("$name: on branch '${current:-detached HEAD}', expected '$branch'")
	dirty=$(git -C "$dir" status --porcelain --untracked-files=no)
	[ -z "$dirty" ] || problems+=("$name: uncommitted changes in $dir:
$(printf '%s\n' "$dirty" | sed 's/^/      /')")
	git -C "$dir" fetch --quiet --tags origin || die "$name: git fetch origin failed in $dir"
	if git -C "$dir" rev-parse -q --verify "refs/remotes/origin/$branch" >/dev/null; then
		behind=$(git -C "$dir" rev-list --count "HEAD..refs/remotes/origin/$branch")
		ahead=$(git -C "$dir" rev-list --count "refs/remotes/origin/$branch..HEAD")
		[ "$behind" = 0 ] || problems+=("$name: $behind commit(s) behind origin/$branch (pull first)")
	else
		problems+=("$name: origin has no branch $branch")
		ahead=0
	fi
	R_AHEAD+=("$ahead")
	R_HEAD+=("$(git -C "$dir" rev-parse HEAD)")

	newest=$(newest_tag "$dir")
	if [ -n "$newest" ]; then
		set -- $newest
		old_core="$1.$2.$3" old_build=$4 old_tag=$5
		set --
	else
		old_core="" old_build=0 old_tag="-"
	fi
	if [ -n "$version_arg" ]; then
		if [ -n "$old_core" ] && [ "$(core_cmp "$version_arg" "$old_core")" != 1 ]; then
			die "$name: --version $version_arg is not greater than the current $old_core ($old_tag)"
		fi
		new_version="$version_arg-1"
	else
		[ -n "$old_core" ] || die "$name: no release tag vX.Y.Z[-N] yet; pass --version X.Y.Z"
		new_version="$old_core-$((old_build + 1))"
	fi
	# The files must not already be ahead of the tags (e.g. 0.13.0 written by
	# hand, newest tag v0.11.0): a next build would go back to 0.11.0-1.
	fv=$(file_version "$dir" "${M_WRITERS[$i]}")
	if [[ $fv =~ ^([0-9]+\.[0-9]+\.[0-9]+) ]] && [ "$(core_cmp "${BASH_REMATCH[1]}" "${new_version%-*}")" = 1 ]; then
		die "$name: the files already say $fv, newer than $new_version; pass --version ${BASH_REMATCH[1]} (or higher)"
	fi
	tag="v$new_version"
	if git -C "$dir" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
		die "$name: tag $tag exists already"
	fi
	if [ -n "$(git -C "$dir" ls-remote --tags origin "refs/tags/$tag" 2>/dev/null)" ]; then
		die "$name: tag $tag exists on origin already"
	fi
	R_OLD+=("$old_tag")
	R_NEW+=("$new_version")
	files=$(changed_files "$dir" "${M_WRITERS[$i]}" | tr '\n' ' ')
	R_FILES+=("${files% }")
done
if [ ${#problems[@]} -gt 0 ]; then
	printf 'release.sh: not releasing:\n' >&2
	printf '  - %s\n' "${problems[@]}" >&2
	rm -rf -- "$work"
	exit 1
fi

# in_run NAME → index into R_* when NAME is released in this run
in_run() {
	local k
	for k in "${!R_IDX[@]}"; do
		[ "${M_NAME[${R_IDX[$k]}]}" = "$1" ] && {
			echo "$k"
			return 0
		}
	done
	return 1
}

# Pins of the released modules that point at commits origin does not have:
# the tag's pipeline would fail to check them out.
pin_warnings=()
check_pin() { # MODULE VAR REPO SHA
	local module=$1 var=$2 repo=$3 sha=$4 rdir k
	rdir=$(repo_dir_of "$repo") || {
		pin_warnings+=("$module pins $var=${sha:0:12} ($repo): $repo is not in $(basename -- "$config"), not checked")
		return 0
	}
	if ! git -C "$rdir" cat-file -e "$sha^{commit}" 2>/dev/null; then
		git -C "$rdir" fetch --quiet origin 2>/dev/null || true
	fi
	if ! git -C "$rdir" cat-file -e "$sha^{commit}" 2>/dev/null; then
		pin_warnings+=("$module pins $var=${sha:0:12}, which $repo ($rdir) does not know")
		return 0
	fi
	[ -z "$(git -C "$rdir" for-each-ref --contains "$sha" refs/remotes/origin 2>/dev/null)" ] || return 0
	if k=$(in_run "$repo") && git -C "$rdir" merge-base --is-ancestor "$sha" HEAD 2>/dev/null; then
		return 0 # pushed earlier in this run (dependency order)
	fi
	pin_warnings+=("$module pins $var=${sha:0:12} ($repo), which is not on origin and not pushed by this run")
}
for k in "${!R_IDX[@]}"; do
	dir=${R_DIR[$k]} name=${M_NAME[${R_IDX[$k]}]}
	wf="$dir/.github/workflows/release.yml"
	if [ -f "$wf" ]; then
		for p in ${P_VAR[@]+"${!P_VAR[@]}"}; do
			sha=$(sed -n "s/^  ${P_VAR[$p]}: *\"\{0,1\}\([0-9a-f]\{40\}\)\"\{0,1\}.*/\1/p" "$wf" | head -n 1)
			[ -z "$sha" ] || check_pin "$name" "${P_VAR[$p]}" "${P_REPO[$p]}" "$sha"
		done
	fi
	if [ -f "$dir/package.json" ]; then
		while IFS=' ' read -r repo sha; do
			[ -n "$sha" ] && check_pin "$name" "package.json" "$repo" "$sha"
		done < <(perl -ne 'print "$1 $2\n" while m{github\.com[:/][^/"]+/([^/"#]+?)(?:\.git)?#([0-9a-f]{40})}g' "$dir/package.json")
	fi
done

print_summary() { # TITLE
	info ""
	info "$1"
	info ""
	printf '  %-22s %-14s    %-14s %-8s %-8s %s\n' module old new branch commits "files changed"
	for k in "${!R_IDX[@]}"; do
		i=${R_IDX[$k]}
		commits=$((R_AHEAD[k] + 1))
		[ -n "${R_FILES[$k]}" ] || commits=${R_AHEAD[$k]}
		printf '  %-22s %-14s -> %-14s %-8s %-8s %s\n' "${M_NAME[$i]}" "${R_OLD[$k]}" "v${R_NEW[$k]}" \
			"${M_BRANCH[$i]}" "$commits" "${R_FILES[$k]:-(none, tag only)}"
	done
	if [ ${#pin_warnings[@]} -gt 0 ]; then
		info ""
		printf 'warning: %s\n' "${pin_warnings[@]}"
	fi
	info ""
}

if [ "$dry_run" = 1 ]; then
	print_summary "Dry run: nothing is written, committed, tagged or pushed."
	for k in "${!R_IDX[@]}"; do
		i=${R_IDX[$k]}
		gate=${M_GATE[$i]}
		[ "$run_gates" = 1 ] && [ "$gate" != "-" ] || gate="(no gates)"
		info "  ${M_NAME[$i]}: gates: $gate; push origin ${M_BRANCH[$i]} v${R_NEW[$k]}"
	done
	rm -rf -- "$work"
	exit 0
fi

# --- local release: write, gates, commit, tag ----------------------------------
# Rolled back by on_exit unless the push started.
done_k=()      # releases with a local commit (when files changed) and tag
pending_k=""   # release whose files are written but not committed yet
pushing=0
rollback() {
	local k dir
	if [ -n "$pending_k" ]; then
		dir=${R_DIR[$pending_k]}
		# shellcheck disable=SC2086 # the file list
		[ -z "${R_FILES[$pending_k]}" ] || git -C "$dir" checkout -q -- ${R_FILES[$pending_k]} || true
	fi
	for k in ${done_k[@]+"${done_k[@]}"}; do
		dir=${R_DIR[$k]}
		git -C "$dir" tag -d "v${R_NEW[$k]}" >/dev/null 2>&1 || true
		git -C "$dir" reset -q --keep "${R_HEAD[$k]}" || warn "could not reset $dir to ${R_HEAD[$k]}"
	done
	[ -z "$pending_k" ] && [ ${#done_k[@]} -eq 0 ] || info "Local release commits and tags removed; nothing was pushed."
}
on_exit() {
	local rc=$?
	if [ "$pushing" = 0 ] && [ "$rc" != 0 ]; then rollback; fi
	rm -rf -- "$work"
	exit "$rc"
}
trap on_exit EXIT
trap 'exit 130' INT TERM

for k in "${!R_IDX[@]}"; do
	i=${R_IDX[$k]}
	name=${M_NAME[$i]} dir=${R_DIR[$k]} new_version=${R_NEW[$k]}
	tag="v$new_version"
	info "== $name: ${R_OLD[$k]} -> $tag"
	pending_k=$k
	write_files "$dir" "${M_WRITERS[$i]}"
	now=$(git -C "$dir" diff --name-only | LC_ALL=C sort | tr '\n' ' ')
	want=""
	# shellcheck disable=SC2086 # the file list
	[ -z "${R_FILES[$k]}" ] || want=$(printf '%s\n' ${R_FILES[$k]} | LC_ALL=C sort | tr '\n' ' ')
	[ "$now" = "$want" ] || die "$name: changed '${now% }', expected '${want% }'"
	gate=${M_GATE[$i]}
	if [ "$run_gates" = 1 ] && [ "$gate" != "-" ]; then
		info "   gates: $gate"
		log="$work/gates-$name.log"
		start=$(date +%s)
		if ! (cd -- "$dir" && bash -c "$gate") >"$log" 2>&1; then
			tail -n 30 "$log" >&2
			die "$name: gates failed ($gate)"
		fi
		info "   gates ok ($(($(date +%s) - start)) s)"
	fi
	if [ -n "${R_FILES[$k]}" ]; then
		# shellcheck disable=SC2086 # the file list
		git -C "$dir" add -- ${R_FILES[$k]}
		# shellcheck disable=SC2086
		git -C "$dir" -c user.name="$author_name" -c user.email="$author_email" \
			commit -q -m "chore(release): $tag" -- ${R_FILES[$k]}
	fi
	git -C "$dir" -c user.name="$author_name" -c user.email="$author_email" tag -a "$tag" -m "$tag"
	done_k+=("$k")
	pending_k=""
	info "   committed and tagged $tag ($(git -C "$dir" rev-parse --short HEAD))"
done

print_summary "Ready to push (local commits and tags are made):"
if [ "$assume_yes" != 1 ]; then
	printf 'Push %d branch(es) and tag(s) to origin? [y/N] ' "${#R_IDX[@]}"
	answer=""
	read -r answer || true
	[ -t 0 ] || info ""
	case $answer in
	y | Y | yes | j | J | ja) ;;
	*)
		info "Not pushed."
		exit 1
		;;
	esac
fi

# --- push ----------------------------------------------------------------------
pushing=1
pushed_k=()
for k in "${!R_IDX[@]}"; do
	i=${R_IDX[$k]}
	name=${M_NAME[$i]} dir=${R_DIR[$k]} branch=${M_BRANCH[$i]} tag="v${R_NEW[$k]}"
	if ! git -C "$dir" push --atomic --quiet origin "refs/heads/$branch:refs/heads/$branch" "refs/tags/$tag"; then
		warn "push of $name failed; pushed so far: ${pushed_k[*]:-nothing}"
		warn "the rest stays local (commits and tags); push them by hand or delete the tags"
		exit 1
	fi
	pushed_k+=("$name")
	info "pushed $name $branch + $tag"
done

info ""
info "The tags start the release pipelines:"
depot=()
for k in "${!R_IDX[@]}"; do
	i=${R_IDX[$k]}
	slug=$(github_slug "${R_DIR[$k]}")
	tag="v${R_NEW[$k]}"
	if [ -n "$slug" ]; then
		printf '  %-22s https://github.com/%s/releases/tag/%s\n' "${M_NAME[$i]}" "$slug" "$tag"
		printf '  %-22s https://github.com/%s/actions/workflows/release.yml\n' "" "$slug"
	else
		printf '  %-22s %s (origin is not on GitHub: %s)\n' "${M_NAME[$i]}" "$tag" "$(git -C "${R_DIR[$k]}" remote get-url origin)"
		slug="${M_NAME[$i]}"
	fi
	[ "${M_PLATFORM[$i]}" = "-" ] || depot+=("$slug@$tag")
done
if [ ${#depot[@]} -gt 0 ]; then
	info ""
	info "Once the pipelines are green, into the depot:"
	info "  depot import github ${depot[*]}"
fi
# The beta server: deploy.sh takes one tag per component.
deploy_env=()
fenrir_tag=""
for i in "${!M_NAME[@]}"; do
	[ "${M_PLATFORM[$i]}" != "-" ] || continue
	dir=$(abs_path "${M_PATH[$i]}")
	newest=$(newest_tag "$dir" 2>/dev/null || true)
	[ -n "$newest" ] || continue
	set -- $newest
	if [ "${M_NAME[$i]}" = fenrir ]; then fenrir_tag=$5; else deploy_env+=("$(deploy_var "${M_NAME[$i]}")=$5"); fi
	set --
done
if [ -n "$fenrir_tag" ]; then
	info "Or onto the beta server:"
	info "  ${deploy_env[*]:-} infra/beta/deploy.sh $fenrir_tag"
fi
