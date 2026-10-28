#!/usr/bin/env bash
# ExecStart of athenos.service: run the host from the release that
# /opt/athenos/current points to *now*.
#
# The host reads config/*.toml, infra/db/migrations and static/ relative to
# its working directory and spawns modules from FENRIR_MODULE_INSTALL_DIR.
# Both are pinned to the resolved release, so switching the symlink during a
# deploy does not change what a running host uses; the next restart does.
set -euo pipefail

current=${ATHENOS_CURRENT:-/opt/athenos/current}
release=$(readlink -f -- "$current") || {
	echo "start-host: $current does not resolve (no release deployed yet?)" >&2
	exit 78 # EX_CONFIG: systemd does not restart-loop on a missing release
}
[ -x "$release/fenrir" ] || {
	echo "start-host: $release/fenrir is missing or not executable" >&2
	exit 78
}
# Files that would load developer settings must never be in a release.
for forbidden in bin/.fenrir-profile config/local.toml secrets/.env; do
	if [ -e "$release/$forbidden" ]; then
		echo "start-host: refusing to start, $release/$forbidden exists" >&2
		exit 78
	fi
done

cd -- "$release"
export FENRIR_MODULE_INSTALL_DIR="$release/modules"
exec "$release/fenrir"
