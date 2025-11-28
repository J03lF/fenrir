use super::completion::{complete_action_targets, complete_force_flags};
use crate::cli::commands::registry::{
    CommandArgument, CommandEntry, CommandHandler, CommandShape, CompletionKind,
};
use crate::services::ServiceActionKind;
use crate::utils::messages::cli::builtins::services::metadata as msg_metadata;

const SERVICE_ALIASES: &[&str] = &["service", "services"];
const MODULE_ALIASES: &[&str] = &["module", "modules"];

pub const ACTION_RESOURCE_COMPLETIONS: &[&str] = &["service", "services", "module", "modules"];

pub const LIST_RESOURCE_COMPLETIONS: &[&str] = &["services", "jobs", "modules"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceActionResource {
    Service,
    Module,
}

impl ServiceActionResource {
    pub fn parse(value: &str) -> Option<Self> {
        let lowered = value.to_ascii_lowercase();
        if SERVICE_ALIASES.contains(&lowered.as_str()) {
            return Some(Self::Service);
        }
        if MODULE_ALIASES.contains(&lowered.as_str()) {
            return Some(Self::Module);
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListResource {
    Services,
    Jobs,
    Modules,
}

impl ListResource {
    pub fn parse(value: &str) -> Option<Self> {
        let lowered = value.to_ascii_lowercase();
        match lowered.as_str() {
            "services" | "service" => Some(Self::Services),
            "jobs" | "job" => Some(Self::Jobs),
            "modules" | "module" => Some(Self::Modules),
            _ => None,
        }
    }

    pub fn default() -> Self {
        Self::Services
    }

    pub fn primary_name(&self) -> &'static str {
        match self {
            Self::Services => "services",
            Self::Jobs => "jobs",
            Self::Modules => "modules",
        }
    }
}

#[derive(Clone, Copy)]
pub struct ServiceActionMetadata {
    pub name: &'static str,
    pub description: &'static str,
    pub synopsis: &'static str,
    pub details: &'static [&'static str],
    pub usage_hint: &'static str,
    pub shape: CommandShape,
}

impl ServiceActionMetadata {
    pub const fn new(
        name: &'static str,
        description: &'static str,
        synopsis: &'static str,
        details: &'static [&'static str],
        usage_hint: &'static str,
        shape: CommandShape,
    ) -> Self {
        Self {
            name,
            description,
            synopsis,
            details,
            usage_hint,
            shape,
        }
    }

    pub fn usage(&self) -> &'static str {
        self.usage_hint
    }

    pub fn as_entry(&self, handler: CommandHandler) -> CommandEntry {
        CommandEntry::with_shape(
            self.name,
            self.description,
            self.synopsis,
            self.details,
            handler,
            self.shape,
        )
    }
}

const SERVICE_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(ACTION_RESOURCE_COMPLETIONS));
const SERVICE_TARGET_ARGUMENT: CommandArgument = CommandArgument::required("target")
    .with_completion(CompletionKind::Dynamic(complete_action_targets));
const SERVICE_FORCE_ARGUMENT: CommandArgument = CommandArgument::optional("flag")
    .with_completion(CompletionKind::Dynamic(complete_force_flags))
    .variadic();

const START_ARGUMENTS: &[CommandArgument] = &[SERVICE_RESOURCE_ARGUMENT, SERVICE_TARGET_ARGUMENT];
const STOP_ARGUMENTS: &[CommandArgument] = &[
    SERVICE_RESOURCE_ARGUMENT,
    SERVICE_TARGET_ARGUMENT,
    SERVICE_FORCE_ARGUMENT,
];
const RESTART_ARGUMENTS: &[CommandArgument] = &[
    SERVICE_RESOURCE_ARGUMENT,
    SERVICE_TARGET_ARGUMENT,
    SERVICE_FORCE_ARGUMENT,
];

pub const START_ACTION: ServiceActionMetadata = ServiceActionMetadata::new(
    "start",
    msg_metadata::START_DESCRIPTION,
    msg_metadata::START_SYNOPSIS,
    msg_metadata::START_DETAILS,
    msg_metadata::START_USAGE,
    CommandShape::new("start", &[], START_ARGUMENTS, &[]),
);

pub const STOP_ACTION: ServiceActionMetadata = ServiceActionMetadata::new(
    "stop",
    msg_metadata::STOP_DESCRIPTION,
    msg_metadata::STOP_SYNOPSIS,
    msg_metadata::STOP_DETAILS,
    msg_metadata::STOP_USAGE,
    CommandShape::new("stop", &[], STOP_ARGUMENTS, &[]),
);

pub const RESTART_ACTION: ServiceActionMetadata = ServiceActionMetadata::new(
    "restart",
    msg_metadata::RESTART_DESCRIPTION,
    msg_metadata::RESTART_SYNOPSIS,
    msg_metadata::RESTART_DETAILS,
    msg_metadata::RESTART_USAGE,
    CommandShape::new("restart", &[], RESTART_ARGUMENTS, &[]),
);

pub const LIST_ARGUMENTS: &[CommandArgument] = &[CommandArgument::optional("resource")
    .with_completion(CompletionKind::Static(LIST_RESOURCE_COMPLETIONS))];

pub const LIST_SHAPE: CommandShape = CommandShape::new("list", &[], LIST_ARGUMENTS, &[]);

pub fn metadata_for_action(action: ServiceActionKind) -> &'static ServiceActionMetadata {
    match action {
        ServiceActionKind::Start => &START_ACTION,
        ServiceActionKind::Stop => &STOP_ACTION,
        ServiceActionKind::Restart => &RESTART_ACTION,
    }
}
