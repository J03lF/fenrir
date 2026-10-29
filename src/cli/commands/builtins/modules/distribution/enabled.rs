//! The depot commands of the console: `search distributions`, `search
//! modules` (depot), `install distribution|module`, `pin`/`unpin module`,
//! `rollback`, `status distribution`. Every install shows its plan first,
//! asks (or takes `--yes`), then runs as a streamed task.

use std::future::Future;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use depot_protocol::rule::VersionPattern;
use depot_protocol::ModuleVersion as DepotVersion;
use depot_protocol::{Channel, Name};
use tokio::runtime::{Handle, Runtime};

use crate::audit::{AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandOutcome, ConfirmationHandler, ConfirmationRequest,
};
use crate::cli::commands::table::Table;
use crate::infra::modules::depot::{DepotHttpClient, DepotSettings};
use crate::services::depot::plan::Restart;
use crate::services::depot::verify::{fetch_verified, newer_distribution};
use crate::services::depot::{
    execute, prepare_distribution, prepare_module, prepare_rollback, update_state, Depot,
    DepotError, DepotSource, DistributionRequest, HostModule, HostPolicy, HostState, Intent,
    ModuleHost, ModuleServiceHost, Prepared, StateStore,
};
use crate::utils::messages::cli::builtins::distribution as msg;

use super::super::ctx::{module_cli_actor, OwnedStreamedWriter};

/// Everything a depot command needs, owned so it can move into tasks.
struct Env {
    settings: DepotSettings,
    client: DepotHttpClient,
    store: StateStore,
    policy: HostPolicy,
    host: ModuleServiceHost,
}

impl Env {
    fn depot(&self) -> Depot<'_> {
        Depot {
            source: &self.client,
            host: &self.host,
            store: &self.store,
            policy: &self.policy,
        }
    }
}

pub(crate) fn depot_configured() -> bool {
    matches!(DepotSettings::from_env(), Ok(Some(_)))
}

/// The depot directory of this host: `<install_dir>/runtime/depot`, next to
/// the module runtime state (on the server a persistent directory linked
/// into every release).
fn depot_dir(deps: &CliDependencies) -> PathBuf {
    PathBuf::from(&deps.config.modules.storage.install_dir)
        .join("runtime")
        .join("depot")
}

fn build_env(deps: &CliDependencies) -> Result<Option<Arc<Env>>, DepotError> {
    let Some(settings) = DepotSettings::from_env()? else {
        return Ok(None);
    };
    let service = deps
        .services
        .module_service()
        .ok_or_else(|| DepotError::Config("the module service is not available".to_string()))?;
    let fenrir = DepotVersion::parse(&deps.config.app.version).map_err(|err| {
        DepotError::Config(format!(
            "app.version `{}` is not a version: {err}",
            deps.config.app.version
        ))
    })?;
    let install_dir = PathBuf::from(&deps.config.modules.storage.install_dir);
    let client = DepotHttpClient::new(&settings)?;
    let host = ModuleServiceHost::new(
        service,
        Arc::clone(&deps.services.db_shell),
        deps.config.runtime.resolve_base_path(),
        install_dir,
    );
    let policy = settings.policy(fenrir);
    Ok(Some(Arc::new(Env {
        settings,
        client,
        store: StateStore::new(depot_dir(deps)),
        policy,
        host,
    })))
}

fn require_env(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<Option<Arc<Env>>> {
    match build_env(deps) {
        Ok(Some(env)) => Ok(Some(env)),
        Ok(None) => {
            writeln!(out, "{}", msg::depot::NOT_CONFIGURED)?;
            Ok(None)
        }
        Err(err) => {
            render_error(out, "Depot settings", &err)?;
            Ok(None)
        }
    }
}

fn block_on<T, F>(future: F) -> Result<T, DepotError>
where
    F: Future<Output = Result<T, DepotError>> + Send + 'static,
    T: Send + 'static,
{
    if let Ok(handle) = Handle::try_current() {
        return tokio::task::block_in_place(|| handle.block_on(future));
    }
    Runtime::new()
        .map_err(|err| DepotError::Io(format!("runtime init failed: {err}")))?
        .block_on(future)
}

fn render_error(out: &mut dyn Write, context: &str, err: &DepotError) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        msg::depot::error(context, &err.to_string(), err.code())
    )
}

fn audit(
    deps: &CliDependencies,
    action: &str,
    target: &str,
    outcome: AuditOutcome,
    metadata: AuditMetadata,
) {
    let event = AuditEvent::builder()
        .actor(module_cli_actor(deps.session_actor()))
        .action(action)
        .target(target.to_string())
        .outcome(outcome)
        .metadata(metadata.insert("transport", "cli"))
        .build();
    match event {
        Ok(event) => {
            if let Err(err) = deps.services.record_audit(event) {
                tracing::warn!(error = %err, "depot audit append failed");
            }
        }
        Err(err) => tracing::warn!(error = %err, "depot audit build failed"),
    }
}

/// Removes `--yes`/`-y` (and returns whether it was there).
fn take_yes<'a>(args: &[&'a str]) -> (Vec<&'a str>, bool) {
    let yes = args.iter().any(|arg| matches!(*arg, "--yes" | "-y"));
    (
        args.iter()
            .copied()
            .filter(|arg| !matches!(*arg, "--yes" | "-y"))
            .collect(),
        yes,
    )
}

fn size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let value = bytes as f64;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", value / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} kB", value / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn short(digest: &str) -> String {
    digest.chars().take(12).collect()
}

/// Collects progress lines of the synchronous phase.
#[derive(Default, Clone)]
struct Lines(Arc<Mutex<Vec<String>>>);

impl Lines {
    fn push(&self, line: String) {
        if let Ok(mut lines) = self.0.lock() {
            lines.push(line);
        }
    }

    fn flush(&self, out: &mut dyn Write) -> io::Result<()> {
        let lines = self
            .0
            .lock()
            .map(|mut lines| std::mem::take(&mut *lines))
            .unwrap_or_default();
        for line in lines {
            writeln!(out, "{line}")?;
        }
        Ok(())
    }
}

// ─── plan ────────────────────────────────────────────────────────────────

fn render_plan(
    out: &mut dyn Write,
    policy: &HostPolicy,
    state: Option<&HostState>,
    prepared: &Prepared,
) -> io::Result<()> {
    let plan = &prepared.plan;
    writeln!(out)?;
    writeln!(out, "{}", msg::plan::title(&plan.title))?;
    match &prepared.intent {
        Intent::Distribution { distribution, .. } => writeln!(
            out,
            "{}",
            msg::plan::signed(
                &distribution.root.key_id.to_string(),
                distribution.root.channel.as_str(),
                &short(&distribution.root.digest.to_hex())
            )
        )?,
        Intent::Module { candidate } => writeln!(
            out,
            "  signed in {} (verified, channel {} or above)",
            candidate.from, policy.channel
        )?,
        Intent::Restore { backup, .. } => writeln!(out, "  local backup {backup}")?,
    }
    if let Some(state) = state {
        if let Some(current) = &state.current {
            let mut now = current.id.clone();
            if !state.overrides.is_empty() {
                now.push_str(&format!(" + {} override(s)", state.overrides.len()));
            }
            writeln!(out, "{}", msg::plan::installed_now(&now))?;
        }
    }
    let mut table = Table::new(
        msg::plan::HEADERS
            .iter()
            .map(|h| (*h).to_string())
            .collect(),
    );
    for entry in &plan.entries {
        let mut action = entry.action.label().to_string();
        if entry.restart != Restart::None {
            action.push_str(" · ");
            action.push_str(entry.restart.label());
        }
        if let Some(note) = &entry.note {
            action.push_str(&format!(" ({note})"));
        }
        let module = match &entry.via {
            Some(via) => format!("{} (via {via})", entry.module),
            None => entry.module.clone(),
        };
        let none = || msg::plan::NONE.to_string();
        table.add_row(vec![
            module,
            entry.rule.clone().unwrap_or_else(none),
            entry
                .installed
                .as_ref()
                .map_or_else(none, ToString::to_string),
            entry.target.as_ref().map_or_else(none, ToString::to_string),
            entry.size.map_or_else(none, size),
            action,
        ]);
    }
    table.render(out, "  ")?;
    let restarts: Vec<String> = plan
        .changes()
        .filter(|entry| entry.restart != Restart::None)
        .map(|entry| format!("{} ({})", entry.module, entry.restart.label()))
        .collect();
    if !restarts.is_empty() {
        writeln!(out, "{}", msg::plan::restarts(&restarts.join(", ")))?;
    }
    let migrations: Vec<String> = plan
        .entries
        .iter()
        .filter(|entry| !entry.migrations.is_empty())
        .map(|entry| format!("{} {}", entry.module, entry.migrations.join(", ")))
        .collect();
    if !migrations.is_empty() {
        writeln!(out, "{}", msg::plan::migrations(&migrations.join("; ")))?;
    }
    for include in &prepared.host_includes {
        writeln!(
            out,
            "{}",
            msg::plan::host(include, &policy.fenrir.to_string())
        )?;
    }
    for line in &prepared.skipped {
        writeln!(out, "{}", msg::plan::skipped(line))?;
    }
    let bundles = plan.changes().filter(|entry| entry.size.is_some()).count();
    if bundles > 0 {
        writeln!(
            out,
            "{}",
            msg::plan::download(
                bundles,
                &size(plan.download_size()),
                &size(prepared.downloaded)
            )
        )?;
    }
    if !plan.problems.is_empty() {
        writeln!(out)?;
        writeln!(out, "{}", msg::plan::PROBLEMS)?;
        for problem in &plan.problems {
            writeln!(out, "  - {problem}")?;
        }
    }
    writeln!(out)?;
    out.flush()
}

/// After the plan: refuse, say there is nothing to do, ask, or (`--yes`) go.
fn confirm_or_run(
    deps: &CliDependencies,
    env: Arc<Env>,
    prepared: Prepared,
    yes: bool,
    command: &str,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    if !prepared.is_installable() {
        return Ok(CommandOutcome::Continue);
    }
    let current = env.store.load().ok().and_then(|state| state.current);
    let same_distribution = match &prepared.intent {
        Intent::Distribution { distribution, .. } => current
            .as_ref()
            .is_some_and(|current| current.id == distribution.id().to_string()),
        _ => true,
    };
    if !prepared.plan.has_changes() && same_distribution {
        writeln!(out, "{}", msg::plan::up_to_date(&prepared.plan.title))?;
        return Ok(CommandOutcome::Continue);
    }
    let prompt = match &prepared.intent {
        Intent::Restore { .. } => msg::plan::confirm_rollback(&prepared.plan.title),
        Intent::Distribution { rollback: true, .. } => {
            msg::plan::confirm_rollback(&prepared.plan.title)
        }
        _ => msg::plan::confirm(&prepared.plan.title),
    };
    let handler = Box::new(RunConfirmation { env, prepared });
    if yes {
        return handler.handle(true, deps, out);
    }
    Ok(CommandOutcome::AwaitConfirmation(
        ConfirmationRequest::new(prompt, handler).with_command_context(command, args),
    ))
}

struct RunConfirmation {
    env: Arc<Env>,
    prepared: Prepared,
}

impl ConfirmationHandler for RunConfirmation {
    fn handle(
        self: Box<Self>,
        accepted: bool,
        deps: &CliDependencies,
        out: &mut dyn Write,
    ) -> io::Result<CommandOutcome> {
        if !accepted {
            writeln!(out, "{}", msg::depot::ABORTED)?;
            return Ok(CommandOutcome::Continue);
        }
        let deps = deps.clone();
        let RunConfirmation { env, prepared } = *self;
        Ok(CommandOutcome::AsyncTask(Box::pin(async move {
            run(deps, env, prepared).await
        })))
    }
}

fn audit_action(intent: &Intent) -> &'static str {
    match intent {
        Intent::Distribution {
            rollback: false, ..
        } => "depot::install_distribution",
        Intent::Distribution { rollback: true, .. } | Intent::Restore { .. } => "depot::rollback",
        Intent::Module { .. } => "depot::install_module",
    }
}

async fn run(
    deps: CliDependencies,
    env: Arc<Env>,
    prepared: Prepared,
) -> io::Result<CommandOutcome> {
    let sink = deps
        .output()
        .ok_or_else(|| io::Error::other("no output sink available"))?;
    let progress_sink = Arc::clone(&sink);
    let progress = move |line: String| progress_sink.push(&format!("{line}\n"));
    let result = execute(&env.depot(), &prepared, &progress).await;
    let mut out = OwnedStreamedWriter::new(sink);
    let action = audit_action(&prepared.intent);
    match result {
        Ok(outcome) => {
            writeln!(out)?;
            writeln!(
                out,
                "{}",
                msg::run::done(&outcome.title, outcome.changed.len())
            )?;
            for line in &outcome.changed {
                writeln!(out, "  {line}")?;
            }
            if let Some(backup) = &outcome.backup {
                writeln!(out, "{}", msg::run::backup(backup))?;
            }
            audit(
                &deps,
                action,
                &outcome.title,
                AuditOutcome::Success,
                AuditMetadata::default()
                    .insert("changed", outcome.changed.join(", "))
                    .insert("migrations", outcome.migrations.join(", ")),
            );
        }
        Err(err) => {
            writeln!(out)?;
            render_error(&mut out, msg::run::FAILED_CONTEXT, &err)?;
            audit(
                &deps,
                action,
                &prepared.plan.title,
                AuditOutcome::Failure,
                AuditMetadata::default()
                    .insert("error_code", err.code())
                    .insert("error", err.to_string()),
            );
        }
    }
    Ok(CommandOutcome::Continue)
}

fn prepared_or_report(
    deps: &CliDependencies,
    out: &mut dyn Write,
    lines: &Lines,
    result: Result<Prepared, DepotError>,
    action: &str,
    target: &str,
) -> io::Result<Option<Prepared>> {
    lines.flush(out)?;
    match result {
        Ok(prepared) => Ok(Some(prepared)),
        Err(err) => {
            render_error(out, "Depot", &err)?;
            writeln!(out, "{}", msg::depot::NOTHING_CHANGED)?;
            audit(
                deps,
                action,
                target,
                AuditOutcome::Failure,
                AuditMetadata::default()
                    .insert("error_code", err.code())
                    .insert("error", err.to_string()),
            );
            Ok(None)
        }
    }
}

// ─── commands ────────────────────────────────────────────────────────────

pub(crate) fn install_distribution(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let (words, yes) = take_yes(args);
    let request = match DistributionRequest::parse(&words) {
        Ok(request) => request,
        Err(message) => {
            writeln!(out, "{message}")?;
            writeln!(out, "{}", msg::usage::INSTALL_DISTRIBUTION)?;
            return Ok(CommandOutcome::Continue);
        }
    };
    let Some(env) = require_env(deps, out)? else {
        return Ok(CommandOutcome::Continue);
    };
    let lines = Lines::default();
    let result = {
        let env = Arc::clone(&env);
        let lines = lines.clone();
        let request = request.clone();
        block_on(async move {
            let progress = move |line: String| lines.push(line);
            prepare_distribution(&env.depot(), &request, &progress).await
        })
    };
    let target = words.join(" ");
    let Some(prepared) = prepared_or_report(
        deps,
        out,
        &lines,
        result,
        "depot::install_distribution",
        &target,
    )?
    else {
        return Ok(CommandOutcome::Continue);
    };
    render_plan(out, &env.policy, env.store.load().ok().as_ref(), &prepared)?;
    let mut context = vec!["distribution"];
    context.extend(args.iter().copied());
    confirm_or_run(deps, env, prepared, yes, "install", &context, out)
}

pub(crate) fn install_module(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let (words, yes) = take_yes(args);
    let Some((name, pattern)) = words.split_first().filter(|(_, rest)| !rest.is_empty()) else {
        writeln!(out, "{}", msg::usage::INSTALL_MODULE)?;
        return Ok(CommandOutcome::Continue);
    };
    let name = match name.parse::<Name>() {
        Ok(name) => name,
        Err(err) => {
            writeln!(out, "`{name}`: {err}")?;
            return Ok(CommandOutcome::Continue);
        }
    };
    let pattern_text = pattern.join(" ");
    let pattern = match pattern_text.parse::<VersionPattern>() {
        Ok(pattern) => pattern,
        Err(err) => {
            writeln!(out, "`{pattern_text}`: {err}")?;
            return Ok(CommandOutcome::Continue);
        }
    };
    let Some(env) = require_env(deps, out)? else {
        return Ok(CommandOutcome::Continue);
    };
    let lines = Lines::default();
    let result = {
        let env = Arc::clone(&env);
        let lines = lines.clone();
        let name = name.clone();
        block_on(async move {
            let progress = move |line: String| lines.push(line);
            prepare_module(&env.depot(), &name, &pattern, &progress).await
        })
    };
    let target = format!("{name} {pattern_text}");
    let Some(prepared) =
        prepared_or_report(deps, out, &lines, result, "depot::install_module", &target)?
    else {
        return Ok(CommandOutcome::Continue);
    };
    render_plan(out, &env.policy, env.store.load().ok().as_ref(), &prepared)?;
    let mut context = vec!["module"];
    context.extend(args.iter().copied());
    confirm_or_run(deps, env, prepared, yes, "install", &context, out)
}

pub(crate) fn rollback(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let (words, yes) = take_yes(args);
    if !words.is_empty() {
        writeln!(out, "{}", msg::usage::ROLLBACK)?;
        return Ok(CommandOutcome::Continue);
    }
    let Some(env) = require_env(deps, out)? else {
        return Ok(CommandOutcome::Continue);
    };
    let lines = Lines::default();
    let result = {
        let env = Arc::clone(&env);
        let lines = lines.clone();
        block_on(async move {
            let progress = move |line: String| lines.push(line);
            prepare_rollback(&env.depot(), &progress).await
        })
    };
    let Some(prepared) =
        prepared_or_report(deps, out, &lines, result, "depot::rollback", "rollback")?
    else {
        return Ok(CommandOutcome::Continue);
    };
    render_plan(out, &env.policy, env.store.load().ok().as_ref(), &prepared)?;
    confirm_or_run(deps, env, prepared, yes, "rollback", args, out)
}

/// `module <name> ...` → `<name> ...`.
fn module_args<'a>(args: &'a [&'a str]) -> Option<&'a [&'a str]> {
    args.split_first()
        .and_then(|(resource, rest)| resource.eq_ignore_ascii_case("module").then_some(rest))
}

pub(crate) fn pin(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let Some([name, version]) = module_args(args) else {
        writeln!(out, "{}", msg::usage::PIN)?;
        return Ok(CommandOutcome::Continue);
    };
    let Ok(version) = DepotVersion::parse(version) else {
        writeln!(out, "`{version}` is not a version")?;
        writeln!(out, "{}", msg::usage::PIN)?;
        return Ok(CommandOutcome::Continue);
    };
    let Some(env) = require_env(deps, out)? else {
        return Ok(CommandOutcome::Continue);
    };
    let modules = {
        let env = Arc::clone(&env);
        block_on(async move { env.host.modules().await.map_err(DepotError::Io) })
    };
    let modules = match modules {
        Ok(modules) => modules,
        Err(err) => {
            render_error(out, "Pin", &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };
    let Some(installed) = modules.iter().find(|module| module.name == *name) else {
        writeln!(out, "{}", msg::pins::not_installed(name))?;
        return Ok(CommandOutcome::Continue);
    };
    if installed.version != version {
        writeln!(
            out,
            "{}",
            msg::pins::other_version(name, &installed.version.to_string(), &version.to_string())
        )?;
        return Ok(CommandOutcome::Continue);
    }
    let module = (*name).to_string();
    match update_state(&env.store, |state| {
        state.pins.insert(module.clone(), version.clone());
        Ok(())
    }) {
        Ok(()) => {
            writeln!(out, "{}", msg::pins::pinned(name, &version.to_string()))?;
            audit(
                deps,
                "depot::pin",
                name,
                AuditOutcome::Success,
                AuditMetadata::default().insert("version", version.to_string()),
            );
        }
        Err(err) => render_error(out, "Pin", &err)?,
    }
    Ok(CommandOutcome::Continue)
}

pub(crate) fn unpin(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let Some([name]) = module_args(args) else {
        writeln!(out, "{}", msg::usage::UNPIN)?;
        return Ok(CommandOutcome::Continue);
    };
    let Some(env) = require_env(deps, out)? else {
        return Ok(CommandOutcome::Continue);
    };
    match update_state(&env.store, |state| Ok(state.pins.remove(*name).is_some())) {
        Ok(true) => {
            writeln!(out, "{}", msg::pins::unpinned(name))?;
            audit(
                deps,
                "depot::unpin",
                name,
                AuditOutcome::Success,
                AuditMetadata::default(),
            );
        }
        Ok(false) => writeln!(out, "{}", msg::pins::not_pinned(name))?,
        Err(err) => render_error(out, "Unpin", &err)?,
    }
    Ok(CommandOutcome::Continue)
}

/// After `uninstall module`: a pin or override of it means nothing anymore.
pub(crate) fn forget_module(deps: &CliDependencies, module: &str) {
    if !depot_configured() {
        return;
    }
    let store = StateStore::new(depot_dir(deps));
    if let Err(err) = update_state(&store, |state| {
        state.pins.remove(module);
        state.overrides.remove(module);
        Ok(())
    }) {
        tracing::warn!(module, error = %err, "depot state not updated after uninstall");
    }
}

/// From the depot: the yank reason of the installed distribution and the
/// newest installable one above it (`id`, channel); or why it did not
/// answer.
type DepotCheck = Result<(Option<String>, Option<(String, String)>), String>;

/// What `status distribution` shows, gathered first so rendering is pure.
struct StatusView {
    channel: Channel,
    url: String,
    state: HostState,
    /// Whether the kept backup undoes the current distribution.
    backup_ok: bool,
    depot: DepotCheck,
    modules: Vec<HostModule>,
}

pub(crate) fn status(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<CommandOutcome> {
    let Some(env) = require_env(deps, out)? else {
        return Ok(CommandOutcome::Continue);
    };
    let state = match env.store.load() {
        Ok(state) => state,
        Err(err) => {
            render_error(out, "Depot state", &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };
    let backup_ok = match (&state.backup, &state.current) {
        (Some(name), Some(current)) => env
            .store
            .load_backup(name)
            .ok()
            .flatten()
            .is_some_and(|record| record.after == current.id),
        _ => false,
    };
    let depot = match &state.current {
        None => Ok((None, None)),
        Some(current) => {
            let id = current.id.clone();
            let env = Arc::clone(&env);
            block_on(async move {
                let parsed = id
                    .parse()
                    .map_err(|err| DepotError::Io(format!("state names `{id}`: {err}")))?;
                let verified = fetch_verified(&env.client, &env.policy.trusted, &parsed).await?;
                let newer = newer_distribution(&env.client, &env.policy, &parsed).await?;
                Ok((
                    verified.root.yanked,
                    newer.map(|newer| (newer.id().to_string(), newer.root.channel.to_string())),
                ))
            })
            .map_err(|err| err.to_string())
        }
    };
    let modules = {
        let env = Arc::clone(&env);
        block_on(async move { env.host.modules().await.map_err(DepotError::Io) })
    }
    .unwrap_or_default();
    render_status(
        out,
        &StatusView {
            channel: env.policy.channel,
            url: env.settings.url.to_string(),
            state,
            backup_ok,
            depot,
            modules,
        },
    )?;
    Ok(CommandOutcome::Continue)
}

fn render_status(out: &mut dyn Write, view: &StatusView) -> io::Result<()> {
    let state = &view.state;
    writeln!(out, "{}", msg::status::TITLE)?;
    let Some(current) = &state.current else {
        writeln!(out, "{}", msg::status::NOTHING)?;
        writeln!(
            out,
            "{}",
            msg::status::follows(view.channel.as_str(), &view.url)
        )?;
        return Ok(());
    };
    writeln!(
        out,
        "{}",
        msg::status::installed(&current.id, &current.channel, &current.installed_at)
    )?;
    writeln!(
        out,
        "{}",
        msg::status::manifest(&short(&current.manifest_sha256), &current.key_id)
    )?;
    writeln!(
        out,
        "{}",
        msg::status::follows(view.channel.as_str(), &view.url)
    )?;
    if !state.overrides.is_empty() {
        let list: Vec<String> = state
            .overrides
            .iter()
            .map(|(name, over)| format!("{name} {} (signed in {})", over.version, over.from))
            .collect();
        writeln!(out, "{}", msg::status::overrides(&list.join(", ")))?;
    }
    if !state.pins.is_empty() {
        let list: Vec<String> = state
            .pins
            .iter()
            .map(|(name, version)| format!("{name} {version}"))
            .collect();
        writeln!(out, "{}", msg::status::pins(&list.join(", ")))?;
    }
    match (&state.previous, view.backup_ok) {
        (Some(previous), true) => {
            writeln!(
                out,
                "{}",
                msg::status::previous(&previous.id, "local backup")
            )?;
        }
        (Some(previous), false) => {
            writeln!(
                out,
                "{}",
                msg::status::previous(&previous.id, "download again")
            )?;
        }
        (None, true) => writeln!(
            out,
            "{}",
            msg::status::previous("the modules from before the depot", "local backup")
        )?,
        (None, false) => {}
    }
    match &view.depot {
        Ok((yanked, newer)) => {
            if let Some(reason) = yanked {
                writeln!(out, "{}", msg::status::yanked(reason))?;
            }
            match newer {
                Some((id, channel)) => writeln!(out, "{}", msg::status::newer(id, channel))?,
                None => writeln!(out, "{}", msg::status::UP_TO_DATE)?,
            }
        }
        Err(err) => writeln!(out, "{}", msg::status::depot_unreachable(err))?,
    }

    let modules = &view.modules;
    let mut table = Table::new(
        msg::status::HEADERS
            .iter()
            .map(|h| (*h).to_string())
            .collect(),
    );
    let mut names: Vec<&String> = current.modules.keys().collect();
    names.extend(modules.iter().map(|module| &module.name));
    names.sort();
    names.dedup();
    for name in names {
        let installed = modules.iter().find(|module| &module.name == name);
        let planned = current.modules.get(name);
        let state_label = match (installed, planned) {
            (None, Some(_)) => "missing".to_string(),
            (Some(module), _) if module.local_sync => "local sync".to_string(),
            (Some(module), _) if state.pins.contains_key(name) => {
                format!("pinned at {}", module.version)
            }
            (Some(module), _) if state.overrides.contains_key(name) => {
                format!("override {}", module.version)
            }
            (Some(module), Some(record)) if record.sha256 == module.sha256 => "ok".to_string(),
            (Some(module), Some(_)) => format!("differs: {} installed", module.version),
            (Some(_), None) => "not in distribution".to_string(),
            (None, None) => continue,
        };
        table.add_row(vec![
            name.clone(),
            installed
                .map(|module| if module.running { "yes" } else { "no" })
                .unwrap_or("-")
                .to_string(),
            planned.map_or_else(|| "-".to_string(), |record| record.version.to_string()),
            state_label,
        ]);
    }
    writeln!(out)?;
    table.render(out, "  ")
}

fn parse_channel_flags<'a>(
    args: &[&'a str],
) -> Result<(Vec<&'a str>, Option<Option<Channel>>), String> {
    let mut words = Vec::new();
    let mut channel: Option<Option<Channel>> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match *arg {
            "--all" => channel = Some(None),
            "--channel" => {
                let value = iter.next().ok_or("--channel needs beta, stable or prod")?;
                channel = Some(Some(
                    value.parse::<Channel>().map_err(|err| err.to_string())?,
                ));
            }
            other => words.push(other),
        }
    }
    Ok((words, channel))
}

pub(crate) fn search_distributions(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let (words, channel) = match parse_channel_flags(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            writeln!(out, "{message}")?;
            writeln!(out, "{}", msg::usage::SEARCH)?;
            return Ok(CommandOutcome::Continue);
        }
    };
    if words.is_empty() {
        writeln!(out, "{}", msg::usage::SEARCH)?;
        return Ok(CommandOutcome::Continue);
    }
    let Some(env) = require_env(deps, out)? else {
        return Ok(CommandOutcome::Continue);
    };
    let minimum = channel.unwrap_or(Some(env.policy.channel));
    let query = words.join(" ");
    let found = {
        let env = Arc::clone(&env);
        let query = query.clone();
        block_on(async move { env.client.search(&query).await })
    };
    let found = match found {
        Ok(found) => found,
        Err(err) => {
            render_error(out, "Search", &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };
    let rows: Vec<_> = found
        .distributions
        .iter()
        .filter(|summary| minimum.is_none_or(|channel| summary.channel >= channel))
        .collect();
    if rows.is_empty() {
        writeln!(out, "{}", msg::search::nothing(&query))?;
    } else {
        let mut table = Table::new(
            msg::search::DIST_HEADERS
                .iter()
                .map(|h| (*h).to_string())
                .collect(),
        );
        for summary in rows {
            let mut modules = summary.modules.len().to_string();
            if !summary.includes.is_empty() {
                let includes: Vec<String> = summary
                    .includes
                    .iter()
                    .map(|i| i.id().to_string())
                    .collect();
                modules.push_str(&format!(" + {}", includes.join(", ")));
            }
            let total = summary
                .modules
                .iter()
                .map(|item| item.size)
                .fold(0, u64::saturating_add);
            table.add_row(vec![
                summary.id().to_string(),
                summary.channel.to_string(),
                summary.published_at.to_string(),
                modules,
                size(total),
                summary
                    .yanked
                    .as_ref()
                    .map(|yank| format!("yanked: {}", yank.reason))
                    .unwrap_or_default(),
            ]);
        }
        table.render(out, "  ")?;
    }
    if let Some(channel) = minimum {
        writeln!(out, "{}", msg::search::filtered(channel.as_str()))?;
    }
    Ok(CommandOutcome::Continue)
}

/// `search modules <name> [range]` against the depot; `None` when no depot
/// is configured (the caller searches the local registry instead).
pub(crate) fn search_modules(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<Option<CommandOutcome>> {
    if !depot_configured() {
        return Ok(None);
    }
    if args.is_empty() {
        writeln!(out, "{}", msg::usage::SEARCH_MODULES)?;
        return Ok(Some(CommandOutcome::Continue));
    }
    let Some(env) = require_env(deps, out)? else {
        return Ok(Some(CommandOutcome::Continue));
    };
    let query = args.join(" ");
    let found = {
        let env = Arc::clone(&env);
        let query = query.clone();
        block_on(async move { env.client.search(&query).await })
    };
    let found = match found {
        Ok(found) => found,
        Err(err) => {
            render_error(out, "Search", &err)?;
            return Ok(Some(CommandOutcome::Continue));
        }
    };
    if found.modules.is_empty() {
        writeln!(out, "{}", msg::search::nothing(&query))?;
        return Ok(Some(CommandOutcome::Continue));
    }
    let mut table = Table::new(
        msg::search::MODULE_HEADERS
            .iter()
            .map(|h| (*h).to_string())
            .collect(),
    );
    for hit in &found.modules {
        for usage in &hit.versions {
            let artifact = &usage.artifact;
            let used_in: Vec<String> = usage
                .used_in
                .iter()
                .map(|reference| {
                    let mut text = format!("{} ({})", reference.distribution, reference.channel);
                    if reference.yanked {
                        text.push_str(" yanked");
                    }
                    text
                })
                .collect();
            table.add_row(vec![
                hit.name.to_string(),
                artifact.version.to_string(),
                artifact.platform.to_string(),
                size(artifact.size),
                if used_in.is_empty() {
                    "-".to_string()
                } else {
                    used_in.join(", ")
                },
                artifact
                    .yanked
                    .as_ref()
                    .map(|yank| format!("yanked: {}", yank.reason))
                    .unwrap_or_default(),
            ]);
        }
    }
    table.render(out, "  ")?;
    Ok(Some(CommandOutcome::Continue))
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/cli/builtins/distribution_render_tests.rs"]
mod tests;
