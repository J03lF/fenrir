use std::io;
use std::path::{Path, PathBuf};

use tokio::fs as tokio_fs;

use crate::domain::module::{ModuleId, ModuleResult, ModuleServiceError, ModuleStorageError};
use crate::utils::messages::services::module::scaffold as scaffold_messages;

use super::types::{ModuleScaffoldOptions, ModuleScaffoldRuntime, ModuleScaffoldSummary};

const GITIGNORE: &str = r"/target
/dist
/build
/node_modules
/.fenrir/dev.env
/.fenrir/dev-agent
/.fenrir/dev-env-*
.DS_Store
";

const RUST_CI_SCRIPT: &str = r"#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all
";

const NODE_CI_SCRIPT: &str = r"#!/usr/bin/env bash
set -euo pipefail

npm run lint
npm test
npm run build
";

const ANGULAR_CI_SCRIPT: &str = r"#!/usr/bin/env bash
set -euo pipefail

npm run lint
npm run test
npm run build
";

const NODE_TSCONFIG: &str = r#"{
  "compilerOptions": {
    "module": "ES2022",
    "moduleResolution": "Node",
    "target": "ES2022",
    "lib": ["ES2022"],
    "esModuleInterop": true,
    "strict": true,
    "skipLibCheck": true,
    "outDir": "dist",
    "rootDir": "src"
  },
  "include": ["src/**/*.ts"]
}
"#;

const ANGULAR_TSCONFIG: &str = r#"{
  "compileOnSave": false,
  "compilerOptions": {
    "baseUrl": "./",
    "outDir": "./dist/out-tsc",
    "forceConsistentCasingInFileNames": true,
    "strict": true,
    "noImplicitOverride": true,
    "noPropertyAccessFromIndexSignature": true,
    "noImplicitReturns": true,
    "noFallthroughCasesInSwitch": true,
    "skipLibCheck": true
  }
}
"#;

const NODE_EMAIL_COMMAND_TS: &str = r#"export interface EmailCommand {
  to: string;
  subject: string;
  body: string;
}
"#;

const NODE_INDEX_TS_TEMPLATE: &str = r#"import { FenrirApp, GatewayClient, DbClient } from 'fenrir-service-kit';
import type { EmailCommand } from './email-command';

export async function bootstrap() {
  const app = await FenrirApp.create();
  const gateway = new GatewayClient(app.context);
  const db = new DbClient(app.context);

  app.router.get('/health', (_req, res) => res.json({ status: 'ok' }));
  app.router.post('/commands/email', async (req, res) => {
    const payload = req.body as EmailCommand;
    await gateway.send('module:notification-hub', payload);
    await db.query('SELECT 1');
    await app.audit.record('module.email.sent', payload);
    res.json({ status: 'queued' });
  });

  await app.listen();
  app.logger.info('{{DISPLAY}} ready');
}

bootstrap().catch((error) => {
  console.error('module failed to start', error);
  process.exit(1);
});
"#;

const ANGULAR_COMPONENT_TS_TEMPLATE: &str = r#"import { Component } from '@angular/core';
import { ControlPlaneService } from 'fenrir-api';

@Component({
  selector: 'fenrir-root',
  template: `
    <section class="wrapper">
      <h1>{{DISPLAY}}</h1>
      <p>Angular control-plane stub talking to Fenrir services.</p>
      <button (click)="refresh()">Refresh diagnostics</button>
      <pre>{{ diagnostics | json }}</pre>
    </section>
  `,
  styles: [
    '.wrapper { padding: 2rem; font-family: sans-serif; }',
    'button { margin-bottom: 1rem; }',
  ],
})
export class AppComponent {
  diagnostics: unknown;

  constructor(private readonly controlPlane: ControlPlaneService) {}

  refresh() {
    this.controlPlane.listServices().subscribe((result) => (
      this.diagnostics = result
    ));
  }
}
"#;

pub(super) async fn generate_module_scaffold(
    module_id: &ModuleId,
    root: PathBuf,
    options: ModuleScaffoldOptions,
) -> ModuleResult<ModuleScaffoldSummary> {
    ModuleScaffolder::new(module_id.clone(), root, options.runtime)
        .build()
        .await
}

struct ModuleScaffolder {
    module_id: ModuleId,
    runtime: ModuleScaffoldRuntime,
    root: PathBuf,
    files: Vec<PathBuf>,
}

impl ModuleScaffolder {
    fn new(module_id: ModuleId, root: PathBuf, runtime: ModuleScaffoldRuntime) -> Self {
        Self {
            module_id,
            runtime,
            root,
            files: Vec::new(),
        }
    }

    async fn build(mut self) -> ModuleResult<ModuleScaffoldSummary> {
        tokio_fs::create_dir_all(&self.root)
            .await
            .map_err(|err| dir_error(&self.root, err))?;
        self.write_file(".gitignore", GITIGNORE).await?;
        match self.runtime {
            ModuleScaffoldRuntime::Rust => self.write_rust_template().await?,
            ModuleScaffoldRuntime::Node => self.write_node_template().await?,
            ModuleScaffoldRuntime::Angular => self.write_angular_template().await?,
        }
        self.files.sort();
        Ok(ModuleScaffoldSummary {
            module_id: self.module_id.clone(),
            runtime: self.runtime,
            root: self.root,
            files: self.files,
        })
    }

    async fn write_rust_template(&mut self) -> ModuleResult<()> {
        self.write_file("Cargo.toml", &rust_cargo_toml(&self.module_id))
            .await?;
        self.write_file("src/main.rs", &rust_main_rs(&self.module_id))
            .await?;
        self.write_file("README.md", &rust_readme(&self.module_id))
            .await?;
        self.write_dev_config(
            Some("target/debug"),
            &["cargo", "run"],
            ".",
            &[("RUST_LOG", "info")],
        )
        .await?;
        self.write_script("scripts/ci.sh", RUST_CI_SCRIPT).await?;
        Ok(())
    }

    async fn write_node_template(&mut self) -> ModuleResult<()> {
        self.write_file("package.json", &node_package_json(&self.module_id))
            .await?;
        self.write_file("tsconfig.json", NODE_TSCONFIG).await?;
        self.write_file("src/index.ts", &node_index_ts(&self.module_id))
            .await?;
        self.write_file("src/email-command.ts", NODE_EMAIL_COMMAND_TS)
            .await?;
        self.write_file("README.md", &node_readme(&self.module_id))
            .await?;
        self.write_dev_config(
            Some("dist"),
            &["npm", "run", "dev"],
            ".",
            &[("NODE_ENV", "development")],
        )
        .await?;
        self.write_script("scripts/ci.sh", NODE_CI_SCRIPT).await?;
        Ok(())
    }

    async fn write_angular_template(&mut self) -> ModuleResult<()> {
        self.write_file("package.json", &angular_package_json(&self.module_id))
            .await?;
        self.write_file("angular.json", &angular_workspace_json())
            .await?;
        self.write_file("tsconfig.base.json", ANGULAR_TSCONFIG)
            .await?;
        self.write_file("apps/fenrir-web/src/main.ts", angular_main_ts())
            .await?;
        self.write_file(
            "apps/fenrir-web/src/app/app.component.ts",
            &angular_component_ts(&self.module_id),
        )
        .await?;
        self.write_file("apps/fenrir-web/src/app/app.module.ts", angular_module_ts())
            .await?;
        self.write_file(
            "projects/fenrir-api/src/lib/control-plane.service.ts",
            angular_control_plane_service_ts(),
        )
        .await?;
        self.write_file(
            "projects/fenrir-api/src/public-api.ts",
            angular_public_api_ts(),
        )
        .await?;
        self.write_file("projects/fenrir-api/ng-package.json", angular_ng_package())
            .await?;
        self.write_file("README.md", &angular_readme(&self.module_id))
            .await?;
        self.write_dev_config(
            Some("dist/fenrir-web"),
            &["npm", "run", "start"],
            ".",
            &[("NG_CLI_ANALYTICS", "false")],
        )
        .await?;
        self.write_script("scripts/ci.sh", ANGULAR_CI_SCRIPT)
            .await?;
        Ok(())
    }

    async fn write_dev_config(
        &mut self,
        output: Option<&str>,
        command: &[&str],
        workdir: &str,
        env: &[(&str, &str)],
    ) -> ModuleResult<()> {
        let content = build_dev_config(
            &self.module_id,
            output,
            command,
            workdir,
            env,
            suggested_port(&self.module_id),
        );
        self.write_file(".fenrir-dev.toml", &content).await
    }

    async fn write_script(&mut self, relative: &str, contents: &str) -> ModuleResult<()> {
        self.write_file(relative, contents).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = self.root.join(relative);
            let metadata = tokio_fs::metadata(&path)
                .await
                .map_err(|err| metadata_error(&path, err))?;
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            tokio_fs::set_permissions(&path, perms)
                .await
                .map_err(|err| permission_error(&path, err))?;
        }
        Ok(())
    }

    async fn write_file(&mut self, relative: impl AsRef<Path>, contents: &str) -> ModuleResult<()> {
        let relative = relative.as_ref();
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            tokio_fs::create_dir_all(parent)
                .await
                .map_err(|err| dir_error(parent, err))?;
        }
        tokio_fs::write(&path, contents)
            .await
            .map_err(|err| io_error(&path, err))?;
        self.files.push(relative.to_path_buf());
        Ok(())
    }
}

fn build_dev_config(
    module_id: &ModuleId,
    output: Option<&str>,
    command: &[&str],
    workdir: &str,
    env: &[(&str, &str)],
    port: u16,
) -> String {
    let display = module_display_name(module_id);
    let mut content = String::new();
    content.push_str("# Generated by fenrir modules scaffold\n");
    if let Some(output) = output {
        content.push_str(&format!("output = \"{}\"\n", output));
    }
    content.push('\n');
    content.push_str("[[services]]\n");
    content.push_str(&format!("id = \"module:{}::service\"\n", module_id));
    content.push_str(&format!("name = \"{display} Service\"\n"));
    content.push_str(&format!("description = \"Dev endpoint for {display}\"\n"));
    content.push_str("kind = \"transport\"\n");
    content.push_str(&format!("endpoint = \"127.0.0.1:{}\"\n", port));
    content.push_str("internal_only = true\n");
    content.push_str("allowed_roles = [\"operator\"]\n");
    content.push_str("required_scopes = [\"module:read\"]\n");
    content.push_str(&format!("route_prefix = \"/{}\"\n", module_id.as_str()));
    content.push_str("health_endpoint = \"/health\"\n");
    content.push_str("access = \"internal\"\n");
    content.push_str("protocols = [\"http\"]\n");
    content.push_str("rate_limit_per_second = 120\n");
    content.push('\n');
    content.push_str("[dev.run]\n");
    content.push_str(&format!("command = [{}]\n", command_array(command)));
    content.push_str(&format!("workdir = \"{}\"\n", workdir));
    content.push_str("auto_restart = true\n");
    content.push_str("auto_start = true\n");
    if !env.is_empty() {
        content.push_str("\n[dev.run.env]\n");
        for (key, value) in env {
            content.push_str(&format!("{} = \"{}\"\n", key, value));
        }
    }
    content
}

fn command_array(command: &[&str]) -> String {
    command
        .iter()
        .map(|value| format!("\"{}\"", value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn module_display_name(module_id: &ModuleId) -> String {
    module_id
        .as_str()
        .split(['-', '_'])
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(first) => format!(
                    "{}{}",
                    first.to_ascii_uppercase(),
                    chars.as_str().to_ascii_lowercase()
                ),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn suggested_port(module_id: &ModuleId) -> u16 {
    let mut hash = 0u32;
    for byte in module_id.as_str().bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u32);
    }
    7000 + (hash % 800) as u16
}

fn rust_cargo_toml(module_id: &ModuleId) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1.0"
axum = {{ version = "0.7", features = ["macros"] }}
fenrir-service-kit = {{ path = "../fenrir-service-kit", default-features = false, features = ["gateway", "db", "telemetry"] }}
serde = {{ version = "1.0", features = ["derive"] }}
serde_json = "1.0"
tokio = {{ version = "1.39", features = ["rt-multi-thread", "macros"] }}
tracing = "0.1"
"#,
        name = module_id.as_str()
    )
}

fn rust_main_rs(module_id: &ModuleId) -> String {
    let display = module_display_name(module_id);
    format!(
        r#"use anyhow::Context;
use axum::{{
    extract::State,
    routing::{{get, post}},
    Json, Router,
}};
use fenrir_service_kit::{{db::DbClient, gateway::GatewayClient, runtime::ServiceContext}};
use serde::{{Deserialize, Serialize}};
use tracing::{{info, instrument}};

#[derive(Debug, Deserialize)]
struct EmailCommand {{
    to: String,
    subject: String,
    body: String,
}}

#[derive(Debug, Serialize)]
struct EmailResponse {{
    status: &'static str,
}}

#[instrument(skip(ctx, payload))]
async fn handle_email(ctx: &ServiceContext, payload: EmailCommand) -> anyhow::Result<()> {{
    GatewayClient::new(ctx)
        .send("module:notification-hub", &payload)
        .await
        .context("notification hub call failed")?;
    DbClient::new(ctx)
        .query("SELECT 1")
        .await
        .context("db connectivity check failed")?;
    ctx.audit()
        .record("module.email.sent", &payload)
        .await
        .context("audit write failed")?;
    Ok(())
}}

async fn email_route(
    State(ctx): State<ServiceContext>,
    Json(command): Json<EmailCommand>,
) -> Result<Json<EmailResponse>, axum::http::StatusCode> {{
    handle_email(&ctx, command)
        .await
        .map(|_| Json(EmailResponse {{ status: "queued" }}))
        .map_err(|err| {{
            tracing::error!(error = %err, "email command failed");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        }})
}}

#[tokio::main]
async fn main() -> anyhow::Result<()> {{
    fenrir_service_kit::telemetry::init_tracing()?;
    let ctx = ServiceContext::current().context("service context")?;
    let router = Router::new()
        .route("/health", get(|| async {{ "ok" }}))
        .route("/commands/email", post(email_route))
        .with_state(ctx.clone());
    info!("{display} ready");
    fenrir_service_kit::http::serve(router, &ctx).await?;
    Ok(())
}}
"#,
        display = display
    )
}

fn rust_readme(module_id: &ModuleId) -> String {
    let display = module_display_name(module_id);
    format!(
        r#"# {display}

This module was generated with `fenrir modules scaffold {module}` and ships with
Fenrir service-kit bindings for Gateway + DbClient usage.

## Getting started

1. Ensure `../fenrir-service-kit` exists next to this folder.
2. Run `cargo run` (Fenrir injects `FENRIR_*` vars during `modules synchronize`).
3. Use `scripts/ci.sh` locally or in CI to run fmt/clippy/tests.

## Next steps

- Extend `EmailCommand` in `src/main.rs` with your business logic.
- Register extra dev services inside `.fenrir-dev.toml`.
- Call `fenrir modules synchronize {module}` to activate the dev override.
"#,
        display = display,
        module = module_id.as_str()
    )
}

fn node_package_json(module_id: &ModuleId) -> String {
    format!(
        r#"{{
  "name": "{name}",
  "version": "0.1.0",
  "type": "module",
  "scripts": {{
    "dev": "ts-node-dev --respawn --transpile-only src/index.ts",
    "build": "tsc -p tsconfig.json",
    "lint": "eslint . --ext .ts",
    "test": "vitest run",
    "start": "node dist/index.js"
  }},
  "dependencies": {{
    "fenrir-service-kit": "file:../fenrir-service-kit/js",
    "zod": "^3.23"
  }},
  "devDependencies": {{
    "@types/node": "^20.0",
    "eslint": "^9.0",
    "ts-node-dev": "^2.0",
    "typescript": "^5.5",
    "vitest": "^1.6"
  }}
}}
"#,
        name = module_id.as_str()
    )
}

fn node_index_ts(module_id: &ModuleId) -> String {
    let display = module_display_name(module_id);
    NODE_INDEX_TS_TEMPLATE.replace("{{DISPLAY}}", &display)
}

fn node_readme(module_id: &ModuleId) -> String {
    let display = module_display_name(module_id);
    format!(
        r#"# {display}

Generated via `fenrir modules scaffold {module}`. The TypeScript entrypoint lives in
`src/index.ts` and already wires Gateway + DbClient helpers from fenrir-service-kit.

## Workflow

- `npm install`
- `npm run dev`
- `fenrir modules synchronize {module}` to activate the dev override
- `scripts/ci.sh` mirrors the fmt/lint/test pipeline
"#,
        display = display,
        module = module_id.as_str()
    )
}

fn angular_package_json(module_id: &ModuleId) -> String {
    format!(
        r#"{{
  "name": "{name}",
  "version": "0.1.0",
  "private": true,
  "scripts": {{
    "start": "ng serve fenrir-web",
    "build": "ng build fenrir-web && ng build fenrir-api",
    "lint": "ng lint",
    "test": "ng test"
  }},
  "dependencies": {{
    "@angular/animations": "^17.3.0",
    "@angular/common": "^17.3.0",
    "@angular/compiler": "^17.3.0",
    "@angular/core": "^17.3.0",
    "@angular/forms": "^17.3.0",
    "@angular/platform-browser": "^17.3.0",
    "@angular/platform-browser-dynamic": "^17.3.0",
    "@angular/router": "^17.3.0",
    "rxjs": "^7.8.0",
    "zone.js": "^0.14.0",
    "fenrir-service-kit": "file:../fenrir-service-kit/js"
  }},
  "devDependencies": {{
    "@angular-devkit/build-angular": "^17.3.0",
    "@angular/cli": "^17.3.0",
    "@angular/compiler-cli": "^17.3.0",
    "typescript": "^5.5.0"
  }}
}}
"#,
        name = module_id.as_str()
    )
}

fn angular_workspace_json() -> String {
    r#"{
  "$schema": "./node_modules/@angular/cli/lib/config/schema.json",
  "version": 1,
  "newProjectRoot": "projects",
  "projects": {
    "fenrir-web": {
      "projectType": "application",
      "root": "apps/fenrir-web",
      "sourceRoot": "apps/fenrir-web/src",
      "architect": {
        "build": {
          "builder": "@angular-devkit/build-angular:browser",
          "options": {
            "outputPath": "dist/fenrir-web",
            "main": "apps/fenrir-web/src/main.ts",
            "index": "apps/fenrir-web/src/index.html",
            "tsConfig": "tsconfig.base.json"
          }
        },
        "serve": {
          "builder": "@angular-devkit/build-angular:dev-server",
          "options": {
            "browserTarget": "fenrir-web:build"
          }
        }
      }
    },
    "fenrir-api": {
      "projectType": "library",
      "root": "projects/fenrir-api",
      "sourceRoot": "projects/fenrir-api/src",
      "architect": {
        "build": {
          "builder": "@angular-devkit/build-angular:ng-packagr",
          "options": {
            "project": "projects/fenrir-api/ng-package.json"
          }
        }
      }
    }
  },
  "defaultProject": "fenrir-web"
}
"#
    .to_string()
}

fn angular_main_ts() -> &'static str {
    r#"import { bootstrapApplication } from '@angular/platform-browser';
import { provideHttpClient } from '@angular/common/http';
import { AppComponent } from './app/app.component';

bootstrapApplication(AppComponent, {
  providers: [provideHttpClient()],
}).catch((err) => console.error(err));
"#
}

fn angular_component_ts(module_id: &ModuleId) -> String {
    let display = module_display_name(module_id);
    ANGULAR_COMPONENT_TS_TEMPLATE.replace("{{DISPLAY}}", &display)
}

fn angular_module_ts() -> &'static str {
    r#"import { NgModule } from '@angular/core';
import { BrowserModule } from '@angular/platform-browser';
import { HttpClientModule } from '@angular/common/http';
import { AppComponent } from './app.component';

@NgModule({
  declarations: [AppComponent],
  imports: [BrowserModule, HttpClientModule],
  bootstrap: [AppComponent],
})
export class AppModule {}
"#
}

fn angular_control_plane_service_ts() -> &'static str {
    r#"import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { Observable } from 'rxjs';

@Injectable({ providedIn: 'root' })
export class ControlPlaneService {
  constructor(private readonly http: HttpClient) {}

  listServices(): Observable<unknown> {
    return this.http.get('/services');
  }
}
"#
}

fn angular_public_api_ts() -> &'static str {
    "export * from './lib/control-plane.service';\n"
}

fn angular_ng_package() -> &'static str {
    r#"{
  "$schema": "./node_modules/ng-packagr/ng-package.schema.json",
  "dest": "dist/fenrir-api",
  "lib": {
    "entryFile": "projects/fenrir-api/src/public-api.ts"
  }
}
"#
}

fn angular_readme(module_id: &ModuleId) -> String {
    let display = module_display_name(module_id);
    format!(
        r#"# {display} (Angular)

This workspace bundles a lightweight Angular app (`fenrir-web`) plus a companion
library (`fenrir-api`). Use it to experiment with operator dashboards without
bootstrapping the CLI.

## Commands

- `npm install`
- `npm run start`
- `npm run build`

`fenrir modules synchronize {module}` registers the dev server described in
`.fenrir-dev.toml` and proxies requests through Fenrir's Gateway.
"#,
        display = display,
        module = module_id.as_str()
    )
}

fn dir_error(path: &Path, err: io::Error) -> ModuleServiceError {
    ModuleServiceError::Storage(ModuleStorageError::Io(
        scaffold_messages::errors::directory_create_failed(path.display(), err),
    ))
}

fn io_error(path: &Path, err: io::Error) -> ModuleServiceError {
    ModuleServiceError::Storage(ModuleStorageError::Io(
        scaffold_messages::errors::write_failed(path.display(), err),
    ))
}

fn metadata_error(path: &Path, err: io::Error) -> ModuleServiceError {
    ModuleServiceError::Storage(ModuleStorageError::Io(
        scaffold_messages::errors::metadata_failed(path.display(), err),
    ))
}

fn permission_error(path: &Path, err: io::Error) -> ModuleServiceError {
    ModuleServiceError::Storage(ModuleStorageError::Io(
        scaffold_messages::errors::permission_failed(path.display(), err),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn rust_scaffold_writes_dev_file() {
        let tmp = tempdir().unwrap();
        let module_id = ModuleId::new("demo-module").unwrap();
        let root = tmp.path().join("demo-module");
        let summary = generate_module_scaffold(
            &module_id,
            root.clone(),
            ModuleScaffoldOptions::new(ModuleScaffoldRuntime::Rust),
        )
        .await
        .unwrap();
        assert!(summary
            .files
            .iter()
            .any(|path| path == Path::new(".fenrir-dev.toml")));
        let contents = tokio_fs::read_to_string(root.join(".fenrir-dev.toml"))
            .await
            .unwrap();
        assert!(contents.contains("demo-module"));
    }

    #[tokio::test]
    async fn node_scaffold_creates_package() {
        let tmp = tempdir().unwrap();
        let module_id = ModuleId::new("node-demo").unwrap();
        let root = tmp.path().join("node-demo");
        let summary = generate_module_scaffold(
            &module_id,
            root.clone(),
            ModuleScaffoldOptions::new(ModuleScaffoldRuntime::Node),
        )
        .await
        .unwrap();
        assert!(summary
            .files
            .iter()
            .any(|path| path == Path::new("package.json")));
        let pkg = tokio_fs::read_to_string(root.join("package.json"))
            .await
            .unwrap();
        assert!(pkg.contains("node-demo"));
    }
}
