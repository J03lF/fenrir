use crate::config::AppConfig;
use crate::utils::messages::prompts as prompt_messages;

use super::constants::{
    CLEAR_SCREEN, COLOR_ACCENT, COLOR_DIM, COLOR_PRIMARY, COLOR_PROMPT, COLOR_RESET, RAW_BANNER,
};

pub fn clear_screen_sequence() -> &'static str {
    CLEAR_SCREEN
}

pub fn banner() -> String {
    format!("{COLOR_PRIMARY}{RAW_BANNER}{COLOR_RESET}")
}

pub fn welcome_line(config: &AppConfig) -> String {
    let version_info = build_version_info(config);
    prompt_messages::welcome_line(
        COLOR_DIM,
        COLOR_ACCENT,
        &config.server.ssh.server_name,
        COLOR_PRIMARY,
        &config.app.name,
        &version_info,
        COLOR_RESET,
    )
}

fn build_version_info(config: &AppConfig) -> String {
    // Use distribution as version if set, otherwise fallback to app.version
    if let Some(dist) = &config.app.distribution {
        if !dist.is_empty() {
            return dist.clone();
        }
    }
    config.app.version.clone()
}

pub fn help_hint() -> String {
    prompt_messages::help_hint(COLOR_DIM, COLOR_RESET)
}

pub(super) fn color_constants() -> ColorPalette {
    ColorPalette {
        accent: COLOR_ACCENT,
        dim: COLOR_DIM,
        primary: COLOR_PRIMARY,
        prompt: COLOR_PROMPT,
        reset: COLOR_RESET,
    }
}

pub(super) struct ColorPalette {
    pub accent: &'static str,
    pub dim: &'static str,
    pub primary: &'static str,
    pub prompt: &'static str,
    pub reset: &'static str,
}
