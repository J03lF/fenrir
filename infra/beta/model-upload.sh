#!/usr/bin/env bash
# Upload an Athenos AI model package (about 1.1 GB) to the beta server.
#
#   infra/beta/model-upload.sh [--host HOST] <local-package-dir>
#   e.g. ~/Documents/personal/modules/models/athenos-quality/0.5.0-beta4
#
# Verifies the package's own SHA256SUMS locally, uploads with rsync
# (resumable), verifies again on the server and installs it read-only to
# /opt/athenos/shared/models/<family>/<version>/ (root-owned). Activate it
# with ATHENOS_QUALITY_MODEL=<family>/<version> in /etc/athenos/fenrir.env
# (or in Admin -> KI), then `sudo systemctl restart athenos`.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=server/common.sh
. "$here/server/common.sh"
load_env_file "${DEPLOY_ENV_FILE:-$HOME/.config/athenos/deploy.env}"
: "${DEPLOY_HOST:=}"
: "${DEPLOY_UPLOAD_ROOT:=/var/tmp/athenos-upload}"

pkg=""
while [ $# -gt 0 ]; do
	case $1 in
	--host)
		DEPLOY_HOST=${2:?--host needs a value}
		shift 2
		;;
	-*) die "unknown option: $1" ;;
	*)
		pkg=$1
		shift
		;;
	esac
done
[ -n "$pkg" ] && [ -d "$pkg" ] || die "usage: model-upload.sh [--host HOST] <local-package-dir>"
[ -n "$DEPLOY_HOST" ] || die "no host: set DEPLOY_HOST in deploy.env or pass --host"
pkg=$(cd -- "$pkg" && pwd)
version=$(basename -- "$pkg")
family=$(basename -- "$(dirname -- "$pkg")")
[[ $version =~ ^[A-Za-z0-9._-]+$ ]] && [[ $family =~ ^[a-z0-9-]+$ ]] || die "unexpected package path: $pkg"
[ -f "$pkg/SHA256SUMS" ] || die "$pkg has no SHA256SUMS"

log "verifying $family/$version locally"
(cd -- "$pkg" && sha256_check SHA256SUMS) || die "local package does not match its SHA256SUMS"

remote="$DEPLOY_UPLOAD_ROOT/model-$family-$version"
ssh -o BatchMode=yes -- "$DEPLOY_HOST" "mkdir -p -m 0700 -- '$remote'"
rsync -a --partial --info=progress2 -e "ssh -o BatchMode=yes" -- "$pkg"/ "$DEPLOY_HOST:$remote/"
# shellcheck disable=SC2029 # the arguments are meant to expand here
ssh -t -- "$DEPLOY_HOST" "set -e
cd '$remote' && sha256sum --check --strict --quiet SHA256SUMS
dest=/opt/athenos/shared/models/$family/$version
sudo install -d -m 0755 -o root -g root /opt/athenos/shared/models/$family
[ ! -e \"\$dest\" ] || { echo \"\$dest exists already; remove it first if you mean to replace it\" >&2; exit 1; }
sudo cp -R '$remote' \"\$dest\"
sudo chown -R root:root \"\$dest\"
sudo chmod -R u=rwX,go=rX \"\$dest\"
rm -rf -- '$remote'
echo \"installed \$dest\""
log "done: set ATHENOS_QUALITY_MODEL=$family/$version and restart athenos to use it"
