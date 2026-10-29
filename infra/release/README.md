# Release-Ablauf

Auf GitHub läuft nichts mehr bei einem gewöhnlichen Push, Pull Request oder nach Zeitplan. Jedes Repo hat genau eine Pipeline, `.github/workflows/release.yml`. Sie startet nur bei einem Tag `v*` oder von Hand („Run workflow“). Die Tags setzt `release.sh` auf dem Mac.

```
release.sh athenos athenos-web                    # jedes genannte Modul: nächste Build-Nummer
release.sh athenos --version 1.3.0                # neue Version, Build beginnt bei 1 → 1.3.0-1
release.sh athenos webcomponents --version webcomponents=0.8.0 --version 1.3.0
                                                  # je Modul: webcomponents 0.8.0-1, athenos 1.3.0-1
release.sh webcomponents                          # eigene Versionslinie: 0.7.6 → 0.7.6-1
release.sh --list                                 # aktuelle Version/Build jedes Moduls
release.sh --dry-run …                            # nur zeigen, nichts ändern
```

Weitere Schalter: `--product-version X.Y.Z[-N]` (Produktversion für die ausgegebene `depot import`-Zeile), `--no-gates` (lokale Tore überspringen), `--yes` (nicht fragen, für Skripte). `contracts` und `webcomponents` gehen als Kurzform für `athenos-contracts` und `athenos-webcomponents`.

### `--version` je Modul

- `--version X.Y.Z` gilt für jedes genannte Modul **ausser** denen mit eigener Versionslinie (`own-version` in `modules.conf`, heute nur webcomponents). Ein solches Modul bekommt seine nächste Build-Nummer, und die Tabelle sagt es in einer `note:`-Zeile.
- `--version NAME=X.Y.Z` gilt nur für dieses Modul (mehrfach möglich) und geht vor dem gemeinsamen Wert. Das Modul muss auch als Modul genannt sein. Nur so bekommt webcomponents eine neue Version.
- webcomponents zählt weiter ab 0.7.6 (Tag `v0.7.6`): `release.sh webcomponents` → `v0.7.6-1`. Am besten ein eigener Lauf, getrennt vom Produkt (siehe „Der erste Release von 0.13.0“).
- athenos-web pinnt webcomponents per Git-SHA in `package.json` (heute `1f2ea46`, auf `origin/0.1.0`; danach kamen nur CI- und README-Änderungen). Ein webcomponents-Release ändert den Pin nicht; einen neuen Stand nimmt athenos-web erst, wenn du den Pin von Hand nachziehst.

## Versionen: `X.Y.Z-N`

- `N` ist die Build-Nummer des Moduls, `N >= 1`. Sie zählt mit jedem Release hoch, je Modul für sich. `-0` und führende Nullen (`-03`) gibt es nicht; das Depot lehnt sie ab.
- Die aktuelle Version ist der neueste Tag `vX.Y.Z-N` des Repos. Sortiert wird nach der Kernversion und dann nach `N` als Zahl (`v0.13.0-10` ist neuer als `v0.13.0-9`), nicht nach den Prerelease-Regeln von Semver.
- Ein alter Tag `vX.Y.Z` zählt als Build 0. Andere Tags (`v0.9.0-rc1`, `vnext`) werden übergangen, `vX.Y.Z-0` und `vX.Y.Z-03` mit einer `note:`-Zeile.
- `--version` nimmt nur `X.Y.Z` ohne führende Nullen; `--version 0.13.0-1` oder `-0` lehnt `release.sh` ab, denn die Build-Nummer zählt es selbst.
- Ohne `--version`: gleicher Kern, `N+1`. Mit `--version X.Y.Z`: `X.Y.Z-1`. Ist `X.Y.Z` nicht grösser als der aktuelle Kern, bricht `release.sh` ab.
- Hat ein Repo noch keinen Tag, braucht es `--version`.
- Stehen in den Dateien schon eine höhere Version als im neuesten Tag (z. B. `0.13.0` in `Cargo.toml`, Tag `v0.11.0`), bricht `release.sh` ohne `--version` ab, statt auf `0.11.0-1` zurückzugehen.

## Was `release.sh` tut

Für jedes genannte Modul, in der Reihenfolge von `modules.conf`:

1. Prüfen: Das Repo hat keine Änderungen an versionierten Dateien (ungetrackte stören nicht). Es steht auf seinem Branch (`0.1.0` für die Module, `main` für contracts, `master` für fenrir) und ist nach `git fetch` nicht hinter `origin`. Der neue Tag gibt es weder lokal noch auf `origin`. Alle Probleme kommen gesammelt, bevor irgendetwas geschrieben wird.
2. Die Version in die Dateien schreiben, die `modules.conf` nennt. Das sind dieselben Stellen wie bei den früheren Release-Commits:
   - Rust-Module: `Cargo.toml`, `Cargo.lock` (nur der Eintrag des Moduls) und `.fenrir-dev.toml`;
   - athenos-api zusätzlich die Version in `openapi/athenos-api.json`;
   - athenos-web: `package.json`, `package-lock.json` und `src/app/data/constants/app.constants.ts` (nicht `whats_new`, das bleibt Handarbeit);
   - webcomponents: `package.json`, `package-lock.json` und die beiden `package.json` unter `projects/` und `dist/`;
   - fenrir: `Cargo.toml`, `Cargo.lock` und `app.version` in `config/default.toml` (Schreiber `app-version=`), damit der Host seine echte Version meldet (Statusseite, `/api/info`, die `fenrir_version`-Prüfung der Module, die Depot-Prüfung `requires.fenrir`). Der Host prüft Bereiche wie `>=0.1.0, <99.99.99` auf dem Kern, also passt `0.13.0-1` (siehe „Build-Nummern im Host“). `release.yml` bricht ab, wenn `app.version` nicht zum Tag passt. `config/prod.toml` setzt weiter `version = "1.0.0"` und überschreibt das auf einem Host, der `prod.toml` lädt (`FENRIR_ENV=prod` ohne `FENRIR_CONFIG_ENV`); dort schaltet die Version auch die externe Identität der SSH-Shell (`>= 1.0.0`). Der Beta-Server nutzt `beta.toml` ohne eigene Version.
   - contracts: nichts, nur der Tag. Jede `Cargo.lock` der Module enthält die Crate-Version der contracts, eine Änderung bräche deren `--locked`-Builds.
3. Die schnellen lokalen Tore laufen (fmt/clippy bzw. `npm run lint`), ausser bei `--no-gates`.
4. Commit `chore(release): vX.Y.Z-N` als Joel Furter <joel.furter@gmail.com>, genau mit diesen Dateien, ohne Trailer.
5. Annotierter Tag `vX.Y.Z-N`.

Danach kommt **eine** Tabelle und **eine** Frage. Erst ein `y` pusht, je Repo Branch und Tag zusammen (`git push --atomic`), in der Reihenfolge der Abhängigkeiten. Jede andere Antwort, ein Fehler, ein rotes Tor oder Ctrl-C vor dem Push entfernt die lokalen Release-Commits und Tags wieder.

Beispiel (der erste 0.13.0-Release in der Sandbox, `--no-gates`, ohne webcomponents):

```
Ready to push (local commits and tags are made):

  module                 old               new            branch   commits  files changed
  athenos-contracts      -              -> v0.13.0-1      main     0        (none, tag only)
  notification-service   v0.11.0        -> v0.13.0-1      0.1.0    1        Cargo.toml Cargo.lock .fenrir-dev.toml
  auth-service           v0.11.0        -> v0.13.0-1      0.1.0    1        Cargo.toml Cargo.lock .fenrir-dev.toml
  athenos                v0.11.0        -> v0.13.0-1      0.1.0    1        Cargo.toml Cargo.lock .fenrir-dev.toml
  athenos-api            v0.12.0        -> v0.13.0-1      0.1.0    1        Cargo.toml Cargo.lock .fenrir-dev.toml openapi/athenos-api.json
  athenos-web            v0.12.1        -> v0.13.0-1      0.1.0    1        package.json package-lock.json src/app/data/constants/app.constants.ts
  fenrir                 v0.12.0        -> v0.13.0-1      master   1        Cargo.toml Cargo.lock config/default.toml

Push 7 branch(es) and tag(s) to origin? [y/N]
```

Danach webcomponents für sich (`release.sh webcomponents`):

```
  athenos-webcomponents  v0.7.6         -> v0.7.6-1       0.1.0    1        package.json package-lock.json projects/athenos-webcomponents/package.json dist/athenos-webcomponents/package.json
```

Wird webcomponents doch im selben Lauf mit `--version 0.13.0` genannt, bekommt es `v0.7.6-1`, und unter der Tabelle steht `note: athenos-webcomponents keeps its own version line: --version 0.13.0 does not apply, next build (a new version: --version athenos-webcomponents=X.Y.Z)`.

`commits` zählt, was der Push mitnimmt (Commits vor `origin` plus den Release-Commit). Eine Pin-Warnung stünde unter der Tabelle, etwa `warning: athenos-api pins ATHENOS_SHA=b0dce7558db8 (athenos), which is not on origin and not pushed by this run`.

Nach dem Push gibt `release.sh` aus:

- die Release- und Actions-Links je Repo;
- je Produkt aus `modules.conf` (`product athenos …`) die Zeile für das Depot, mit den Schaltern der echten CLI (fenrir-depot d4b/d4c, `crates/depot-cli/src/import.rs`) und nur den Modulen des Produkts, die im Lauf sind (nicht contracts, webcomponents, fenrir):

  ```
  depot import github --product athenos --version <product-version> --repo fenrir-modules/athenos@v0.13.0-1 --repo fenrir-modules/athenos-api@v0.13.0-1 --repo fenrir-modules/athenos-web@v0.13.0-1 --repo fenrir-modules/auth-service@v0.13.0-1 --repo fenrir-modules/notification-service@v0.13.0-1 [--from athenos-<prev>]
  ```

  `<product-version>` füllst du aus, oder `--product-version 0.13.0` setzt sie gleich ein. Fehlen Module des Produkts im Lauf, steht `--from athenos-<prev>` ohne Klammern da (und eine Zeile, welche fehlen): erst `--from` übernimmt deren Regeln aus der vorigen Distribution. Ein `--dry-run` zeigt die Zeile auch schon;
- die Zeile für den Beta-Server mit einem `TAG_<KOMPONENTE>` je Repo, denn die Build-Nummern unterscheiden sich: `TAG_ATHENOS=v0.13.0-2 … infra/beta/deploy.sh v0.13.0-1`.

### Pins

Was ein Release-Lauf auf GitHub auscheckt, steht als SHA in `release.yml` (`MODULE_KIT_SHA`, `CONTRACTS_SHA`, `ATHENOS_SHA`, `DEPOT_SHA`) oder als Git-Pin in `package.json` (`github.com/<owner>/<repo>.git#<sha>`). `release.sh` warnt, wenn ein gepinnter Commit weder auf `origin` liegt noch im selben Lauf vorher gepusht wird. Der Release-Lauf auf GitHub fände ihn sonst nicht. Die Pins setzt du weiter von Hand; `release.sh` ändert sie nicht.

## `modules.conf`

Die Tabelle neben dem Skript ordnet jedem Modul Repo-Pfad, Branch, Versionsstellen, Plattform des Bündels und lokale Tore zu. `own-version` markiert Module mit eigener Versionslinie, `product` nennt ein Depot-Produkt und seine Module. Die Reihenfolge der `module`-Zeilen ist die Push-Reihenfolge:

contracts → notification-service → auth-service → athenos → athenos-api → athenos-web, webcomponents, fenrir.

`sibling` nennt Repos, die nur gepinnt werden (module-kit, fenrir-depot), `pin` die Pin-Variablen. Eine andere Tabelle geht über `RELEASE_CONFIG=/pfad/modules.conf`, eine andere Wurzel über `RELEASE_ROOT`.

## Die Pipelines

Alle Actions sind auf einen vollen Commit-SHA gepinnt. `permissions:` steht oben auf `contents: read`; nur der Release-Job darf `contents: write`, und den Token bekommt nur der Schritt, der den GitHub-Release anlegt. Bei einem Tag muss die Version in den Dateien zum Tag passen, sonst bricht der Lauf ab.

| Repo | Tore (einmal je Release) | Am GitHub-Release |
|---|---|---|
| athenos | fmt, clippy, Tests (mit Wegwerf-Postgres im Runner), Radar-Modell, cargo-deny | `athenos-<v>-linux-x86_64.tar.gz`, `SHA256SUMS` |
| athenos-api | fmt, clippy, Tests (Routen-Parität gegen `ATHENOS_SHA`), cargo-deny | `athenos-api-<v>-linux-x86_64.tar.gz`, `SHA256SUMS` |
| auth-service, notification-service | fmt, clippy, Tests, cargo-deny | `<modul>-<v>-linux-x86_64.tar.gz`, `SHA256SUMS` |
| athenos-web | Pins, Lint, Unit-Tests (Chromium), `npm audit --audit-level=high`; im Release-Job Produktions-Build ohne Warnungen und Budgets | `athenos-web-<v>-any.tar.gz`, `SHA256SUMS` |
| athenos-webcomponents | Lint, `npm audit`, Token-Tests, Unit-Tests (mit und ohne zone.js), `build:lib`, `dist/` passt, Storybook axe (Linux) und Screenshots (macOS) | `athenos-webcomponents-<v>.tgz` (`npm pack`), `SHA256SUMS`; kein npm-Registry-Publish |
| athenos-contracts | fmt, clippy, Tests, cargo-deny | nur der Release (kein Artefakt) |
| fenrir | rustfmt, clippy, Tests (alle Features, Postgres im Runner), ohne depot-Feature, cargo-deny, `infra/beta/tests/check.sh` | `fenrir-<v>-linux-x86_64.tar.gz` (das Host-Bündel aus `infra/beta/ci/package-host.sh`), `SHA256SUMS` |

Das Modul-Bündel hat das Format, das der Depot-Installer des Hosts (`src/services/depot/bundle.rs`) und der Beta-Server (`infra/beta/server/athenos-release.sh`) entpacken:

- ein gzip-Tar mit einem obersten Ordner `<modul>/`;
- darin das Binary `<modul>/<modul>` (Web: `dist/`), `.fenrir/runtime.toml` samt `.fenrir/`, `migrations/`, `.fenrir-meta/manifest.json` (gleiche id und Version) und `install.json`, `BUILDINFO.json`, `FILES.SHA256SUMS`;
- nur Dateien und Ordner, keine Links.

Den obersten Ordner mit gleichem Namen wie das Binary kann der Host seit dem D4-Fix entpacken. Ein Schritt in `release.yml` prüft das Bündel nach diesen Regeln, bevor es hochgeladen wird.

### Secrets auf GitHub

- `SIBLING_REPOS_TOKEN` in jedem Repo ausser athenos-contracts und athenos-webcomponents: ein Fine-grained Token, nur **Contents: Read-only**, für die Repos module-kit, athenos-contracts, athenos, athenos-webcomponents (für das npm-Install von athenos-web) und fenrir-depot. Fehlt er, bricht der erste Schritt mit einer klaren Meldung ab.
- Den GitHub-Release legt das eingebaute `github.token` an. Ein npm-Token braucht es nicht, weil nichts in eine Registry geht.

## Der erste Release von 0.13.0

1. Die Integrationszweige (`q-rel` auf `q-i013`, `q-d4`) in `0.1.0`, `main` und `master` mergen, damit die neuen Workflows im Release sind. Das Depot (`d4c`) in seinen Hauptzweig mergen und pushen, denn `DEPOT_SHA` in fenrirs `release.yml` zeigt auf `99b5e32` (d4c).
2. Die Pins zeigen auf Commits der Integrationszweige (`CONTRACTS_SHA` eb62489, `ATHENOS_SHA` b0dce75). Sie sind noch nicht auf GitHub, aber contracts und athenos werden im selben Lauf vorher gepusht. `MODULE_KIT_SHA` liegt auf `origin`; `DEPOT_SHA` erst nach Schritt 1. `release.sh` warnt, falls etwas fehlt.
3. Dann das Produkt und den Host, ohne webcomponents:

   ```
   infra/release/release.sh --dry-run --version 0.13.0 --product-version 0.13.0 contracts notification-service auth-service athenos athenos-api athenos-web fenrir
   infra/release/release.sh --version 0.13.0 --product-version 0.13.0 contracts notification-service auth-service athenos athenos-api athenos-web fenrir
   ```

   Jedes Modul bekommt `v0.13.0-1`. Das ist grösser als alle alten Tags (`v0.12.1`, `v0.11.0`, `v0.1.0-18` usw.). Die alten `v0.1.0-N` liest `release.sh` als 0.1.0 Build N; sie stören nicht.
4. webcomponents für sich, auf seiner Linie: `infra/release/release.sh webcomponents` → `v0.7.6-1`.
5. Wenn die Pipelines grün sind, die ausgegebene Zeile `depot import github --product athenos --version 0.13.0 --repo … [--from athenos-<prev>]` ausführen (`--from` nur, wenn es eine vorige Distribution gibt, deren Regeln gelten sollen).
6. Ein Fix danach: `release.sh athenos` → `v0.13.0-2`, nur für athenos.

## Tests

`tests/release-test.sh` prüft `release.sh` in einer Sandbox unter `$TMPDIR`. Die Fixture-Repos haben lokale Bare-Repos als `origin`, git läuft mit einer Wegwerf-Konfiguration, ohne Netz und ohne GitHub. Geprüft werden:

- `--list`, die nächste Build-Nummer, ein alter Tag als Build 0, numerische Sortierung, `--version` mit Neustart bei 1;
- das Ablehnen einer gleichen oder kleineren Version, von `--version` mit Build-Nummer, `-0` oder führenden Nullen, einer `--product-version …-0`, eines schmutzigen Repos, eines Repos hinter `origin` oder auf dem falschen Branch; Tags `-0`/`-03` werden übergangen;
- `--version` je Modul, die eigene Versionslinie von webcomponents (`0.7.6` → `0.7.6-1`), `app.version` in fenrirs `config/default.toml`;
- die `depot import github`-Zeile (Schalter, nur Produktmodule, `--from`);
- `--dry-run`, die Tabelle, die eine Frage und der Rückbau bei Nein, ohne Antwort und bei einem roten Tor;
- Autor, Committer, Tagger und Commit-Text, genau die Versionsdateien, Tag und Branch im Bare-Remote;
- die Pin-Warnung und die Push-Reihenfolge.

Dazu kommt `tests/workflow-lint.sh`: nur `release.yml`, nur Tag- und manuelle Auslöser, `permissions` minimal, Actions per SHA. Es nutzt actionlint, wenn installiert, sonst PyYAML oder Rubys Psych. `infra/beta/tests/check.sh` ruft beides auf.

`tests/release-test.sh --real /opt/fenrir/development/worktrees/rel` klont zusätzlich die `q-rel`-Checkouts in Bare-Remotes. Es spielt dort den ersten 0.13.0-Release durch (`--no-gates --yes`, dann webcomponents für sich) und prüft die echten Workflows.

## Build-Nummern im Host, im Depot und in npm

`X.Y.Z-N` ist für Semver eine Vorabversion (`0.13.0-1 < 0.13.0`, `>=0.1.0, <99.99.99` trifft `0.13.0-1` nicht). Host und Depot lesen es deshalb nicht als Semver:

- **Depot** (d4c, `depot_protocol::ModuleVersion`): Ordnung nach Kern, dann `N`; Regeln (`athenos.depot.toml`, `requires`) prüfen den Kern. `=X.Y.Z` ist genau Build 0.
- **Host** (fenrir): dieselben Regeln. Die Depot-Pfade nutzen `depot_protocol::ModuleVersion` (`install … <range>`, `install module`, `pin`, „newer“ in `status`, die eigene Version für `requires.fenrir`). Ausserhalb des Depots ordnet `domain::module::ModuleVersion` nach Build-Nummer, und `version_req_matches` prüft `fenrir_version` der Module, Abhängigkeitsbereiche und die Distributionsziele der Registry auf dem Kern. Ein `fenrir_version` wie `>=0.1.0, <99.99.99` (so schreiben es die `release.yml` der Module) passt damit auf einen Host `0.13.0-1`; nichts muss Vorabversionen eigens zulassen. Eine echte Vorabversion (`0.7.0-preview3`) behält die Semver-Regeln.
- **npm**: Bereiche wie `^0.7.6` treffen `0.7.6-1` nicht. Die Pakete hängen per Git-SHA aneinander (athenos-web → webcomponents, contracts), daher betrifft es heute nichts; ein Bereich auf ein Build bräuchte die Vorabversion ausdrücklich (`>=0.7.6-1`).
