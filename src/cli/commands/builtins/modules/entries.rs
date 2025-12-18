use std::sync::Arc;

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandShape, CommandSubcommand,
    CompletionContext, CompletionKind,
};
use crate::utils::messages::cli::builtins::modules as msg_modules;
use tracing::warn;

use super::ctx::run_module_call;
use super::handlers;

const MODULE_ALIASES: &[&str] = &["module"];

const MODULE_ID_ARGUMENT: CommandArgument = CommandArgument {
    name: "module",
    optional: false,
    variadic: false,
    completion: CompletionKind::Dynamic(complete_module_ids),
};

const MODULE_ID_OPTIONAL_ARGUMENT: CommandArgument = CommandArgument {
    name: "module",
    optional: true,
    variadic: false,
    completion: CompletionKind::Dynamic(complete_module_ids),
};

const MODULE_LOG_TAIL_ARGUMENT: CommandArgument = CommandArgument {
    name: "--tail",
    optional: true,
    variadic: false,
    completion: CompletionKind::Static(&["--tail"]),
};

const MODULE_RUNTIME_ARGUMENT: CommandArgument = CommandArgument {
    name: "--runtime",
    optional: true,
    variadic: false,
    completion: CompletionKind::Static(&["--runtime"]),
};

const MODULE_RESOURCE_SINGULAR_OPTIONS: &[&str] = &["module"];
const MODULE_RESOURCE_PLURAL_OPTIONS: &[&str] = &["modules"];
const MODULE_RESOURCE_SINGULAR_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(MODULE_RESOURCE_SINGULAR_OPTIONS));
const MODULE_RESOURCE_PLURAL_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(MODULE_RESOURCE_PLURAL_OPTIONS));
const MODULE_PATTERN_ARGUMENT: CommandArgument = CommandArgument::optional("pattern");

const SEARCH_ARGUMENTS: &[CommandArgument] =
    &[MODULE_RESOURCE_PLURAL_ARGUMENT, MODULE_PATTERN_ARGUMENT];
const SEARCH_SHAPE: CommandShape = CommandShape::new("search", &[], SEARCH_ARGUMENTS, &[]);

const DISTRIBUTION_RESOURCE_OPTIONS: &[&str] = &["distribution", "distributions"];
const DISTRIBUTION_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(DISTRIBUTION_RESOURCE_OPTIONS));

const INSTALL_ARGUMENTS: &[CommandArgument] = &[DISTRIBUTION_RESOURCE_ARGUMENT];
const INSTALL_SHAPE: CommandShape =
    CommandShape::new("install", &["import"], INSTALL_ARGUMENTS, &[]);

const UNINSTALL_ARGUMENTS: &[CommandArgument] =
    &[MODULE_RESOURCE_SINGULAR_ARGUMENT, MODULE_ID_ARGUMENT];
const UNINSTALL_SHAPE: CommandShape =
    CommandShape::new("uninstall", &["remove"], UNINSTALL_ARGUMENTS, &[]);

const CHECK_ARGUMENTS: &[CommandArgument] = &[MODULE_RESOURCE_PLURAL_ARGUMENT];
const CHECK_SHAPE: CommandShape = CommandShape::new("check", &[], CHECK_ARGUMENTS, &[]);

const SYNCHRONIZE_ARGUMENTS: &[CommandArgument] =
    &[MODULE_RESOURCE_SINGULAR_ARGUMENT, MODULE_ID_ARGUMENT];
const SYNCHRONIZE_SHAPE: CommandShape =
    CommandShape::new("synchronize", &["sync"], SYNCHRONIZE_ARGUMENTS, &[]);

const RELEASE_ARGUMENTS: &[CommandArgument] =
    &[MODULE_RESOURCE_SINGULAR_ARGUMENT, MODULE_ID_ARGUMENT];
const RELEASE_SHAPE: CommandShape = CommandShape::new("release", &[], RELEASE_ARGUMENTS, &[]);

const SCAFFOLD_ARGUMENTS: &[CommandArgument] = &[
    MODULE_RESOURCE_SINGULAR_ARGUMENT,
    MODULE_ID_ARGUMENT,
    MODULE_RUNTIME_ARGUMENT,
];
const SCAFFOLD_SHAPE: CommandShape = CommandShape::new("scaffold", &[], SCAFFOLD_ARGUMENTS, &[]);

pub fn search_command() -> CommandEntry {
    CommandEntry::with_shape(
        "search",
        msg_modules::search::DESCRIPTION,
        msg_modules::search::SYNOPSIS,
        msg_modules::search::DETAILS,
        handlers::handle_search_command,
        SEARCH_SHAPE,
    )
}

pub fn install_command() -> CommandEntry {
    CommandEntry::with_shape(
        "install",
        msg_modules::install_distribution::DESCRIPTION,
        msg_modules::install_distribution::SYNOPSIS,
        msg_modules::install_distribution::DETAILS,
        handlers::handle_install_command,
        INSTALL_SHAPE,
    )
}

pub fn synchronize_command() -> CommandEntry {
    CommandEntry::with_shape(
        "synchronize",
        msg_modules::synchronize::DESCRIPTION,
        msg_modules::synchronize::SYNOPSIS,
        msg_modules::synchronize::DETAILS,
        handlers::handle_synchronize_command,
        SYNCHRONIZE_SHAPE,
    )
}

pub fn release_command() -> CommandEntry {
    CommandEntry::with_shape(
        "release",
        msg_modules::release::DESCRIPTION,
        msg_modules::release::SYNOPSIS,
        msg_modules::release::DETAILS,
        handlers::handle_release_command,
        RELEASE_SHAPE,
    )
}

pub fn scaffold_command() -> CommandEntry {
    CommandEntry::with_shape(
        "scaffold",
        msg_modules::scaffold::DESCRIPTION,
        msg_modules::scaffold::SYNOPSIS,
        msg_modules::scaffold::DETAILS,
        handlers::handle_scaffold_command,
        SCAFFOLD_SHAPE,
    )
}

pub fn uninstall_command() -> CommandEntry {
    CommandEntry::with_shape(
        "uninstall",
        msg_modules::uninstall::DESCRIPTION,
        msg_modules::uninstall::SYNOPSIS,
        msg_modules::uninstall::DETAILS,
        handlers::handle_uninstall_command,
        UNINSTALL_SHAPE,
    )
}

pub fn check_command() -> CommandEntry {
    CommandEntry::with_shape(
        "check",
        msg_modules::check::DESCRIPTION,
        msg_modules::check::SYNOPSIS,
        msg_modules::check::DETAILS,
        handlers::handle_check_command,
        CHECK_SHAPE,
    )
}

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "modules",
        msg_modules::command::DESCRIPTION,
        msg_modules::command::SYNOPSIS,
        msg_modules::command::DETAILS,
        handlers::handle,
        MODULES_SHAPE,
    )
}

pub(super) fn resolve_module_subcommand(alias: &str) -> Option<&'static str> {
    MODULE_SUBCOMMANDS
        .iter()
        .find(|entry| entry.name == alias || entry.aliases.contains(&alias))
        .map(|entry| entry.name)
}

pub(super) fn available_subcommands() -> String {
    MODULE_SUBCOMMANDS
        .iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn complete_module_ids(deps: &CliDependencies, _ctx: &CompletionContext<'_>) -> Vec<String> {
    let Some(service) = deps.services.module_service() else {
        return Vec::new();
    };

    match run_module_call(Arc::clone(&service), |svc| async move {
        svc.list_installed().await
    }) {
        Ok(installed) => {
            let mut ids: Vec<_> = installed
                .into_iter()
                .map(|module| module.manifest.id)
                .collect();
            ids.sort();
            ids
        }
        Err(err) => {
            warn!(target = "cli::completion", error = %err, "module id completion failed");
            Vec::new()
        }
    }
}

const MODULE_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new("list", &[], &[], msg_modules::subcommands::LIST_DESC),
    CommandSubcommand::new("search", &[], &[], msg_modules::subcommands::SEARCH_DESC),
    CommandSubcommand::new(
        "info",
        &[],
        &[MODULE_ID_ARGUMENT],
        msg_modules::subcommands::INFO_DESC,
    ),
    CommandSubcommand::new(
        "install-distribution",
        &["install_distribution"],
        &[],
        msg_modules::subcommands::INSTALL_DESC,
    ),
    CommandSubcommand::new(
        "synchronize",
        &["sync"],
        &[MODULE_ID_ARGUMENT],
        msg_modules::subcommands::SYNCHRONIZE_DESC,
    ),
    CommandSubcommand::new(
        "release",
        &[],
        &[MODULE_ID_ARGUMENT],
        msg_modules::subcommands::RELEASE_DESC,
    ),
    CommandSubcommand::new(
        "scaffold",
        &[],
        &[MODULE_ID_ARGUMENT, MODULE_RUNTIME_ARGUMENT],
        msg_modules::subcommands::SCAFFOLD_DESC,
    ),
    CommandSubcommand::new(
        "release-dev-overrides",
        &["release_dev_overrides", "release-dev"],
        &[],
        msg_modules::subcommands::RELEASE_DEV_OVERRIDES_DESC,
    ),
    CommandSubcommand::new(
        "uninstall",
        &["remove"],
        &[MODULE_ID_ARGUMENT],
        msg_modules::subcommands::UNINSTALL_DESC,
    ),
    CommandSubcommand::new(
        "check-updates",
        &["check_updates"],
        &[],
        msg_modules::subcommands::CHECK_DESC,
    ),
    CommandSubcommand::new(
        "log",
        &[],
        &[MODULE_ID_ARGUMENT, MODULE_LOG_TAIL_ARGUMENT],
        msg_modules::subcommands::LOG_DESC,
    ),
    CommandSubcommand::new(
        "env",
        &[],
        &[MODULE_ID_ARGUMENT],
        msg_modules::subcommands::ENV_DESC,
    ),
    CommandSubcommand::new(
        "services",
        &[],
        &[MODULE_ID_OPTIONAL_ARGUMENT],
        msg_modules::subcommands::SERVICES_DESC,
    ),
    CommandSubcommand::new(
        "start",
        &[],
        &[MODULE_ID_ARGUMENT],
        msg_modules::subcommands::START_DESC,
    ),
    CommandSubcommand::new(
        "stop",
        &[],
        &[MODULE_ID_ARGUMENT],
        msg_modules::subcommands::STOP_DESC,
    ),
    CommandSubcommand::new(
        "restart",
        &[],
        &[MODULE_ID_ARGUMENT],
        msg_modules::subcommands::RESTART_DESC,
    ),
    CommandSubcommand::new(
        "stop-all",
        &["stop_all"],
        &[],
        msg_modules::subcommands::STOP_ALL_DESC,
    ),
];

const MODULES_SHAPE: CommandShape =
    CommandShape::new("modules", MODULE_ALIASES, &[], MODULE_SUBCOMMANDS);
