#!/usr/bin/env bash
# Package the Fenrir host for the Athenos beta server (used by
# .github/workflows/release.yml; runs on ubuntu-24.04 with GNU tar).
#
#   infra/beta/ci/package-host.sh <release-bin-dir> <version> <out-dir>
#
# Output in <out-dir>:
#   fenrir-<version>-x86_64-unknown-linux-gnu.tar.gz   one top directory fenrir/
#   SHA256SUMS                                          of the tarball
# The tarball holds the three binaries (fenrir, fenrir-module-kit,
# fenrirctl), config/default.toml + prod.toml + beta.toml (from
# infra/beta/config), infra/db/migrations, infra/beta (server scripts,
# units, configs), static/, BUILDINFO.json and FILES.SHA256SUMS.
# Never: bin/.fenrir-profile, config/local.toml, config/dev.toml, secrets/.
# Reproducible: tracked files only, sorted, fixed mtime (SOURCE_DATE_EPOCH),
# owner 0:0, gzip without name/time.
set -euo pipefail

bin_dir=${1:?release bin dir (target/<triple>/release)}
version=${2:?version}
out=${3:?out dir}
: "${SOURCE_DATE_EPOCH:?SOURCE_DATE_EPOCH must be set}"
: "${TARGET_TRIPLE:=x86_64-unknown-linux-gnu}"
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.+-]+)?$ ]] || {
	echo "version must be semver: $version" >&2
	exit 1
}

root=$(git rev-parse --show-toplevel)
cd -- "$root"
mkdir -p -- "$out"
out=$(cd -- "$out" && pwd)
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
pkg="$stage/fenrir"
mkdir -p -- "$pkg/config"

for b in fenrir fenrir-module-kit fenrirctl; do
	install -m 0755 -- "$bin_dir/$b" "$pkg/$b"
done
install -m 0644 -- config/default.toml config/prod.toml "$pkg/config/"
install -m 0644 -- infra/beta/config/beta.toml "$pkg/config/beta.toml"

# Tracked files only (nothing local or secret can slip in).
copy_tracked() { # path...
	git ls-files -z -- "$@" | while IFS= read -r -d '' f; do
		install -D -m "$([ -x "$f" ] && echo 0755 || echo 0644)" -- "$f" "$pkg/$f"
	done
}
copy_tracked infra/db/migrations infra/beta static

for forbidden in bin/.fenrir-profile config/local.toml config/dev.toml secrets; do
	[ ! -e "$pkg/$forbidden" ] || {
		echo "refusing to package $forbidden" >&2
		exit 1
	}
done

commit=$(git rev-parse HEAD)
jq -n --arg component fenrir --arg version "$version" --arg commit "$commit" \
	--arg target "$TARGET_TRIPLE" --arg rustc "$(rustc -V)" --arg cargo "$(cargo -V)" \
	--argjson sde "$SOURCE_DATE_EPOCH" --arg lock "$(sha256sum Cargo.lock | cut -d' ' -f1)" \
	'{component: $component, version: $version, commit: $commit, target: $target,
	  rustc: $rustc, cargo: $cargo, source_date_epoch: $sde, cargo_lock_sha256: $lock}' >"$pkg/BUILDINFO.json"

(cd -- "$pkg" && find . -type f ! -name FILES.SHA256SUMS -printf '%P\0' | LC_ALL=C sort -z |
	xargs -0 sha256sum >FILES.SHA256SUMS)

archive="fenrir-$version-$TARGET_TRIPLE.tar.gz"
tar --sort=name --format=gnu --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
	--mode='u+rw,go-w' -C "$stage" -cf - fenrir | gzip -n -9 >"$out/$archive"
(cd -- "$out" && sha256sum -- "$archive" >SHA256SUMS)
cat -- "$out/SHA256SUMS"
