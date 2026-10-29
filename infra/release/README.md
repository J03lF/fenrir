# Release-Ablauf

Auf GitHub läuft nichts mehr bei einem gewöhnlichen Push, Pull Request oder nach Zeitplan. Jedes Repo hat genau eine Pipeline, `.github/workflows/release.yml`. Sie startet nur bei einem Tag `v*` oder von Hand („Run workflow“). Die Tags setzt `release.sh` auf dem Mac.

```
release.sh athenos athenos-web          # jedes genannte Modul: nächste Build-Nummer
release.sh athenos --version 1.3.0      # neue Version, Build beginnt bei 1 → 1.3.0-1
release.sh --list                       # aktuelle Version/Build jedes Moduls
release.sh --dry-run …                  # nur zeigen, nichts ändern
```

Weitere Schalter: `--no-gates` (lokale Tore überspringen), `--yes` (nicht fragen, für Skripte). `contracts` und `webcomponents` gehen als Kurzform für `athenos-contracts` und `athenos-webcomponents`.

## Versionen: `X.Y.Z-N`

- `N` ist die Build-Nummer des Moduls. Sie zählt mit jedem Release hoch, je Modul für sich.
- Die aktuelle Version ist der neueste Tag `vX.Y.Z-N` des Repos. Sortiert wird nach der Kernversion und dann nach `N` als Zahl (`v0.13.0-10` ist neuer als `v0.13.0-9`), nicht nach den Prerelease-Regeln von Semver.
- Ein alter Tag `vX.Y.Z` zählt als Build 0. Andere Tags (`v0.9.0-rc1`, `vnext`) werden übergangen.
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
   - fenrir: `Cargo.toml` und `Cargo.lock`. `config/default.toml` (`app.version`) bleibt, weil der Host damit die `fenrir_version`-Bereiche der Module prüft und ein `-N` dort nicht passt.
   - contracts: nichts, nur der Tag. Jede `Cargo.lock` der Module enthält die Crate-Version der contracts, eine Änderung bräche deren `--locked`-Builds.
3. Die schnellen lokalen Tore laufen (fmt/clippy bzw. `npm run lint`), ausser bei `--no-gates`.
4. Commit `chore(release): vX.Y.Z-N` als Joel Furter <joel.furter@gmail.com>, genau mit diesen Dateien, ohne Trailer.
5. Annotierter Tag `vX.Y.Z-N`.

Danach kommt **eine** Tabelle und **eine** Frage. Erst ein `y` pusht, je Repo Branch und Tag zusammen (`git push --atomic`), in der Reihenfolge der Abhängigkeiten. Jede andere Antwort, ein Fehler, ein rotes Tor oder Ctrl-C vor dem Push entfernt die lokalen Release-Commits und Tags wieder.

Beispiel (der erste 0.13.0-Release in der Sandbox, `--no-gates`):

```
Ready to push (local commits and tags are made):

  module                 old               new            branch   commits  files changed
  athenos-contracts      -              -> v0.13.0-1      main     0        (none, tag only)
  notification-service   v0.11.0        -> v0.13.0-1      0.1.0    1        Cargo.toml Cargo.lock .fenrir-dev.toml
  auth-service           v0.11.0        -> v0.13.0-1      0.1.0    1        Cargo.toml Cargo.lock .fenrir-dev.toml
  athenos                v0.11.0        -> v0.13.0-1      0.1.0    1        Cargo.toml Cargo.lock .fenrir-dev.toml
  athenos-api            v0.12.0        -> v0.13.0-1      0.1.0    1        Cargo.toml Cargo.lock .fenrir-dev.toml openapi/athenos-api.json
  athenos-web            v0.12.1        -> v0.13.0-1      0.1.0    1        package.json package-lock.json src/app/data/constants/app.constants.ts
  athenos-webcomponents  v0.7.6         -> v0.13.0-1      0.1.0    1        package.json package-lock.json projects/athenos-webcomponents/package.json dist/athenos-webcomponents/package.json
  fenrir                 v0.12.0        -> v0.13.0-1      master   1        Cargo.toml Cargo.lock

Push 8 branch(es) and tag(s) to origin? [y/N]
```

`commits` zählt, was der Push mitnimmt (Commits vor `origin` plus den Release-Commit). Eine Pin-Warnung stünde unter der Tabelle, etwa `warning: athenos-api pins ATHENOS_SHA=b0dce7558db8 (athenos), which is not on origin and not pushed by this run`.

Nach dem Push gibt `release.sh` aus:

- die Release- und Actions-Links je Repo;
- die Zeile für das Depot, z. B. `depot import github fenrir-modules/athenos@v0.13.0-1 …` (der Befehl entsteht gerade in einer eigenen Runde; die Zeile ist ein Vorschlag);
- die Zeile für den Beta-Server mit einem `TAG_<KOMPONENTE>` je Repo, denn die Build-Nummern unterscheiden sich: `TAG_ATHENOS=v0.13.0-2 … infra/beta/deploy.sh v0.13.0-1`.

### Pins

Was ein Release-Lauf auf GitHub auscheckt, steht als SHA in `release.yml` (`MODULE_KIT_SHA`, `CONTRACTS_SHA`, `ATHENOS_SHA`, `DEPOT_SHA`) oder als Git-Pin in `package.json` (`github.com/<owner>/<repo>.git#<sha>`). `release.sh` warnt, wenn ein gepinnter Commit weder auf `origin` liegt noch im selben Lauf vorher gepusht wird. Der Release-Lauf auf GitHub fände ihn sonst nicht. Die Pins setzt du weiter von Hand; `release.sh` ändert sie nicht.

## `modules.conf`

Die Tabelle neben dem Skript ordnet jedem Modul Repo-Pfad, Branch, Versionsstellen, Plattform des Bündels und lokale Tore zu. Die Reihenfolge der `module`-Zeilen ist die Push-Reihenfolge:

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

1. Die Integrationszweige (`q-rel` auf `q-i013`, `q-d4`) in `0.1.0`, `main` und `master` mergen, damit die neuen Workflows im Release sind.
2. Die Pins zeigen auf Commits der Integrationszweige (`CONTRACTS_SHA` eb62489, `ATHENOS_SHA` b0dce75). Sie sind noch nicht auf GitHub, aber contracts und athenos werden im selben Lauf vorher gepusht. `MODULE_KIT_SHA` und `DEPOT_SHA` liegen schon auf `origin`. `release.sh` warnt, falls doch etwas fehlt.
3. Dann:

   ```
   infra/release/release.sh --dry-run --version 0.13.0 contracts notification-service auth-service athenos athenos-api athenos-web webcomponents fenrir
   infra/release/release.sh --version 0.13.0 contracts notification-service auth-service athenos athenos-api athenos-web webcomponents fenrir
   ```

   Jedes Modul bekommt `v0.13.0-1`. Das ist grösser als alle alten Tags (`v0.12.1`, `v0.11.0`, `v0.1.0-18` usw.). Die alten `v0.1.0-N` liest `release.sh` als 0.1.0 Build N; sie stören nicht.
4. Ein Fix danach: `release.sh athenos` → `v0.13.0-2`, nur für athenos.

## Tests

`tests/release-test.sh` prüft `release.sh` in einer Sandbox unter `$TMPDIR`. Die Fixture-Repos haben lokale Bare-Repos als `origin`, git läuft mit einer Wegwerf-Konfiguration, ohne Netz und ohne GitHub. Geprüft werden:

- `--list`, die nächste Build-Nummer, ein alter Tag als Build 0, numerische Sortierung, `--version` mit Neustart bei 1;
- das Ablehnen einer gleichen oder kleineren Version, eines schmutzigen Repos, eines Repos hinter `origin` oder auf dem falschen Branch;
- `--dry-run`, die Tabelle, die eine Frage und der Rückbau bei Nein, ohne Antwort und bei einem roten Tor;
- Autor, Committer, Tagger und Commit-Text, genau die Versionsdateien, Tag und Branch im Bare-Remote;
- die Pin-Warnung und die Push-Reihenfolge.

Dazu kommt `tests/workflow-lint.sh`: nur `release.yml`, nur Tag- und manuelle Auslöser, `permissions` minimal, Actions per SHA. Es nutzt actionlint, wenn installiert, sonst PyYAML oder Rubys Psych. `infra/beta/tests/check.sh` ruft beides auf.

`tests/release-test.sh --real /opt/fenrir/development/worktrees/rel` klont zusätzlich die `q-rel`-Checkouts in Bare-Remotes. Es spielt dort den ersten 0.13.0-Release durch (`--no-gates --yes`) und prüft die echten Workflows.

## Hinweis zu Semver

`X.Y.Z-N` ist für Semver eine Vorabversion. Das hat Folgen:

- `0.13.0-1` gilt als kleiner als `0.13.0`.
- Regeln wie `0.13.*` oder `>=0.1.0, <99.99.99` treffen `0.13.0-1` nicht. Das betrifft das Depot (Regeln in `athenos.depot.toml`), den Host (`fenrir_version` der Module, Abhängigkeitsbereiche) und npm-Bereiche.
- Ein exakter Pin auf eine Version funktioniert.

Solange diese Stellen die Build-Nummer nicht eigens behandeln, muss man dort Vorabversionen ausdrücklich zulassen.
