use crate::config::AppConfig;
use crate::utils::messages::prompts as prompt_messages;

use super::context::{PromptContext, PromptSet};
use super::theme::color_constants;

pub fn prompt_set(config: &AppConfig, context: &PromptContext) -> PromptSet {
    let colors = color_constants();
    let app = &config.app.name;

    let main_transport = prompt_messages::main_prompt(
        colors.accent,
        &context.role,
        colors.prompt,
        &context.transport,
        colors.primary,
        &context.user,
        &context.host,
        colors.dim,
        app,
        colors.reset,
    );
    let db_transport = prompt_messages::db_prompt(
        colors.accent,
        &context.role,
        colors.prompt,
        &context.transport,
        colors.primary,
        &context.user,
        &context.host,
        colors.accent,
        colors.dim,
        app,
        colors.reset,
    );
    let main_cli = wrap_non_print_sequences(&main_transport);
    let db_cli = wrap_non_print_sequences(&db_transport);
    PromptSet {
        main_cli,
        main_transport,
        db_cli,
        db_transport,
    }
}

#[allow(clippy::while_let_on_iterator)]
fn wrap_non_print_sequences(input: &str) -> String {
    let mut result = String::with_capacity(input.len() + 8);
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            result.push('\u{1}');
            result.push(ch);
            while let Some(next) = chars.next() {
                result.push(next);
                if next == 'm' {
                    break;
                }
            }
            result.push('\u{2}');
        } else {
            result.push(ch);
        }
    }
    result
}
