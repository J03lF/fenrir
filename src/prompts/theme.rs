use crate::config::AppConfig;

use super::constants::{
    BOLD, CLEAR_SCREEN, COLOR_ACCENT, COLOR_BRAND, COLOR_DIM, COLOR_LABEL, COLOR_PRIMARY,
    COLOR_PROMPT, COLOR_RESET, COLOR_SUBTLE, COLOR_VALUE,
};
use super::context::PromptContext;

const BOX_WIDTH: usize = 52;

pub fn clear_screen_sequence() -> &'static str {
    CLEAR_SCREEN
}

pub fn banner(config: &AppConfig, context: &PromptContext) -> String {
    let palette = color_constants();
    build_banner(config, context, &palette)
}

pub fn welcome_line(config: &AppConfig) -> String {
    let version = build_version_info(config);
    format!(
        "{dim}▸ {bold}{brand}fenrir{reset}{dim} · {server} · {version}{reset}\n",
        dim = COLOR_DIM,
        bold = BOLD,
        brand = COLOR_BRAND,
        reset = COLOR_RESET,
        server = config.server.ssh.server_name,
        version = version,
    )
}

fn build_version_info(config: &AppConfig) -> String {
    if let Some(dist) = &config.app.distribution {
        if !dist.is_empty() {
            return dist.clone();
        }
    }
    config.app.version.clone()
}

pub fn help_hint() -> String {
    format!(
        "{dim}  type {accent}help{dim} for available commands{reset}\n",
        dim = COLOR_DIM,
        accent = COLOR_ACCENT,
        reset = COLOR_RESET,
    )
}

pub(super) fn color_constants() -> ColorPalette {
    ColorPalette {
        accent: COLOR_ACCENT,
        dim: COLOR_DIM,
        primary: COLOR_PRIMARY,
        prompt: COLOR_PROMPT,
        reset: COLOR_RESET,
        brand: COLOR_BRAND,
        subtle: COLOR_SUBTLE,
        label: COLOR_LABEL,
        value: COLOR_VALUE,
        bold: BOLD,
    }
}

pub(super) struct ColorPalette {
    pub accent: &'static str,
    pub dim: &'static str,
    pub primary: &'static str,
    pub prompt: &'static str,
    pub reset: &'static str,
    pub brand: &'static str,
    pub subtle: &'static str,
    pub label: &'static str,
    pub value: &'static str,
    pub bold: &'static str,
}

/// Pad content to fixed width (accounts for visible chars only)
fn pad_line(content: &str, visible_len: usize) -> String {
    let padding = BOX_WIDTH.saturating_sub(visible_len);
    format!("{}{}", content, " ".repeat(padding))
}

fn build_banner(config: &AppConfig, context: &PromptContext, p: &ColorPalette) -> String {
    let version = build_version_info(config);
    let profile = config
        .app
        .profile
        .as_deref()
        .filter(|v| !v.is_empty())
        .unwrap_or("—");

    let server = &config.server.ssh.server_name;
    let user = &context.user;
    let role = &context.role;

    let mut out = String::with_capacity(800);

    // Horizontal line helper
    let hline = "─".repeat(BOX_WIDTH);

    // Top border
    out.push_str(&format!(
        "\n{s}  ╭{line}╮{r}\n",
        s = p.subtle,
        line = hline,
        r = p.reset
    ));

    // Empty row
    out.push_str(&format!(
        "{s}  │{r}{pad}{s}│{r}\n",
        s = p.subtle,
        r = p.reset,
        pad = " ".repeat(BOX_WIDTH)
    ));

    // Header: FENRIR › server
    let header_visible = 3 + 6 + 3 + server.len(); // "   FENRIR › {server}"
    let header_content = format!(
        "   {b}{brand}FENRIR{r} {l}›{r} {v}{server}{r}",
        b = p.bold,
        brand = p.brand,
        r = p.reset,
        l = p.label,
        v = p.value,
        server = server
    );
    out.push_str(&format!(
        "{s}  │{r}{content}{s}│{r}\n",
        s = p.subtle,
        r = p.reset,
        content = pad_line(&header_content, header_visible)
    ));

    // Empty row
    out.push_str(&format!(
        "{s}  │{r}{pad}{s}│{r}\n",
        s = p.subtle,
        r = p.reset,
        pad = " ".repeat(BOX_WIDTH)
    ));

    // Divider
    out.push_str(&format!(
        "{s}  ├{line}┤{r}\n",
        s = p.subtle,
        line = hline,
        r = p.reset
    ));

    // Row 1: version + profile
    // "   version   1.4.0            profile   local_dev"
    let row1 = format!(
        "   {l}version{r}   {v}{ver:<12}{r}   {l}profile{r}   {v}{prof}{r}",
        l = p.label,
        r = p.reset,
        v = p.value,
        ver = version,
        prof = profile
    );
    let row1_visible = 3 + 7 + 3 + 12 + 3 + 7 + 3 + profile.len();
    out.push_str(&format!(
        "{s}  │{r}{content}{s}│{r}\n",
        s = p.subtle,
        r = p.reset,
        content = pad_line(&row1, row1_visible)
    ));

    // Row 2: user + role
    let row2 = format!(
        "   {l}user{r}      {a}{usr:<12}{r}   {l}role{r}      {a}{rl}{r}",
        l = p.label,
        r = p.reset,
        a = p.accent,
        usr = user,
        rl = role
    );
    let row2_visible = 3 + 4 + 6 + 12 + 3 + 4 + 6 + role.len();
    out.push_str(&format!(
        "{s}  │{r}{content}{s}│{r}\n",
        s = p.subtle,
        r = p.reset,
        content = pad_line(&row2, row2_visible)
    ));

    // Empty row
    out.push_str(&format!(
        "{s}  │{r}{pad}{s}│{r}\n",
        s = p.subtle,
        r = p.reset,
        pad = " ".repeat(BOX_WIDTH)
    ));

    // Bottom border
    out.push_str(&format!(
        "{s}  ╰{line}╯{r}\n",
        s = p.subtle,
        line = hline,
        r = p.reset
    ));

    out
}
