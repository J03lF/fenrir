#!/usr/bin/env bash
# Deploy an Athenos release to the beta server, from the Mac.
#
#   infra/beta/deploy.sh [options] <release-tag | artifact-dir>
#
#   <release-tag>    download the Linux artifacts of that tag from GitHub
#                    (gh release download; one release per component repo,
#                    TAG_<COMPONENT> in deploy.env overrides the tag per repo)
#   <artifact-dir>   use local artifacts: the six tarballs with their
#                    SHA256SUMS files (flat, or one sub-directory per component)
#   --host HOST      SSH destination (default DEPLOY_HOST from deploy.env)
#   --restore-db-on-failure
#                    if the new release is unhealthy, also restore the database
#                    from the pre-deploy dump (DISCARDS writes after the dump).
#                    Default: only the release is rolled back.
#   --dry-run        verify and stage locally, upload nothing
#
# Steps: verify every SHA256SUMS locally, stage the six tarballs with one
# combined SHA256SUMS, upload with rsync, then on the server (sudo)
# athenos-release.sh deploy: verify again, unpack to
# /opt/athenos/releases/<id>, pre-deploy pg_dump, atomic switch of
# /opt/athenos/current, restart, wait for health, automatic rollback to the
# previous release if unhealthy, prune to 5 releases.
# Settings: ~/.config/athenos/deploy.env (template env/deploy.env.example).
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=server/common.sh
. "$here/server/common.sh"
load_env_file "${DEPLOY_ENV_FILE:-$HOME/.config/athenos/deploy.env}"

: "${DEPLOY_HOST:=}"
: "${DEPLOY_UPLOAD_ROOT:=/var/tmp/athenos-upload}"
: "${REPO_FENRIR:=J03lF/fenrir}"
: "${REPO_ATHENOS:=fenrir-modules/athenos}"
: "${REPO_ATHENOS_API:=fenrir-modules/athenos-api}"
: "${REPO_AUTH_SERVICE:=fenrir-modules/auth-service}"
: "${REPO_NOTIFICATION_SERVICE:=fenrir-modules/notification-service}"
: "${REPO_ATHENOS_WEB:=fenrir-modules/athenos-web}"

COMPONENTS="fenrir athenos athenos-api auth-service notification-service athenos-web"

restore_flag=""
dry_run=0
source_arg=""
while [ $# -gt 0 ]; do
	case $1 in
	--host)
		DEPLOY_HOST=${2:?--host needs a value}
		shift 2
		;;
	--restore-db-on-failure)
		restore_flag=--restore-db-on-failure
		shift
		;;
	--dry-run)
		dry_run=1
		shift
		;;
	-h | --help)
		sed -n '2,28p' "$0"
		exit 0
		;;
	-*) die "unknown option: $1" ;;
	*)
		[ -z "$source_arg" ] || die "only one release tag or directory"
		source_arg=$1
		shift
		;;
	esac
done
[ -n "$source_arg" ] || die "usage: deploy.sh [options] <release-tag | artifact-dir>"
[ "$dry_run" = 1 ] || [ -n "$DEPLOY_HOST" ] || die "no host: set DEPLOY_HOST in deploy.env or pass --host"

work=$(mktemp -d "${TMPDIR:-/tmp}/athenos-deploy.XXXXXX")
trap 'rm -rf -- "$work"' EXIT
fetched="$work/fetched"
stage="$work/stage"
mkdir -p "$fetched" "$stage"

var_suffix() { printf '%s' "$1" | tr 'a-z-' 'A-Z_'; }

if [ -d "$source_arg" ]; then
	id_base="local"
	cp -R "$source_arg"/. "$fetched"/
else
	tag=$source_arg
	[[ $tag =~ ^[A-Za-z0-9._+-]+$ ]] || die "invalid tag: $tag"
	command -v gh >/dev/null 2>&1 || die "gh (GitHub CLI) is needed for a release tag"
	id_base=$tag
	for comp in $COMPONENTS; do
		suffix=$(var_suffix "$comp")
		repo_var="REPO_$suffix"
		tag_var="TAG_$suffix"
		repo=${!repo_var}
		comp_tag=${!tag_var:-$tag}
		log "downloading $comp $comp_tag from $repo"
		mkdir -p "$fetched/$comp"
		gh release download "$comp_tag" --repo "$repo" --dir "$fetched/$comp" \
			--pattern '*-x86_64-unknown-linux-gnu.tar.gz' --pattern '*-static.tar.gz' --pattern 'SHA256SUMS' ||
			die "download of $comp $comp_tag from $repo failed"
	done
fi

# Verify every SHA256SUMS in place; remember which tarballs were covered.
covered="$work/covered"
: >"$covered"
while IFS= read -r sums; do
	dir=$(dirname -- "$sums")
	(cd -- "$dir" && sha256_check "$(basename -- "$sums")") || die "checksum mismatch in $sums"
	awk '{print $2}' "$sums" | sed 's/^\*//' | while IFS= read -r name; do printf '%s/%s\n' "$dir" "$name"; done >>"$covered"
	log "verified $sums"
done < <(find "$fetched" -maxdepth 2 -type f \( -name 'SHA256SUMS' -o -name '*.SHA256SUMS' \))

# Exactly one verified tarball per component.
for comp in $COMPONENTS; do
	found=$(find "$fetched" -maxdepth 2 -type f -name "$comp-[0-9]*.tar.gz")
	n=$(printf '%s' "$found" | grep -c . || true)
	[ "$n" = 1 ] || die "expected one tarball for $comp, found $n"
	grep -Fxq -- "$found" "$covered" || die "$(basename -- "$found") is not covered by a verified SHA256SUMS"
	cp -- "$found" "$stage/"
done

# Server tools from this checkout (the same revision as this script).
cp -- "$here/server/athenos-release.sh" "$here/server/common.sh" "$stage/"
(
	cd -- "$stage"
	for f in *; do printf '%s  %s\n' "$(sha256_of "$f")" "$f"; done >"$work/SHA256SUMS"
)
mv -- "$work/SHA256SUMS" "$stage/SHA256SUMS"
id="$id_base-$(utc_stamp)"
id=$(printf '%s' "$id" | tr -c 'A-Za-z0-9._+-' '_')
log "staged release $id:"
sed 's/^/    /' "$stage/SHA256SUMS" >&2

if [ "$dry_run" = 1 ]; then
	log "dry run: nothing uploaded"
	exit 0
fi

remote="$DEPLOY_UPLOAD_ROOT/$id"
if [ -n "$restore_flag" ]; then
	cat >&2 <<'EOF'

  NOTE: --restore-db-on-failure: if the new release is unhealthy, the
  database is ALSO restored from the pre-deploy dump. Writes after the dump
  are then no longer live (restore.sh keeps the old database under a new
  name).

EOF
fi
log "uploading to $DEPLOY_HOST:$remote"
ssh -o BatchMode=yes -- "$DEPLOY_HOST" "mkdir -p -m 0700 -- '$DEPLOY_UPLOAD_ROOT' '$remote'"
rsync -a --partial --chmod=D700,F600 -e "ssh -o BatchMode=yes" -- "$stage"/ "$DEPLOY_HOST:$remote/"

log "deploying on $DEPLOY_HOST (sudo may ask for your password)"
rc=0
# shellcheck disable=SC2029 # the arguments are meant to expand here
ssh -t -- "$DEPLOY_HOST" "sudo bash '$remote/athenos-release.sh' deploy '$remote' '$id' $restore_flag" || rc=$?
if [ "$rc" = 0 ]; then
	ssh -o BatchMode=yes -- "$DEPLOY_HOST" "rm -rf -- '$remote'" || true
	log "release $id is live"
else
	log "deploy failed (exit $rc); upload kept at $DEPLOY_HOST:$remote for inspection"
fi
exit "$rc"
