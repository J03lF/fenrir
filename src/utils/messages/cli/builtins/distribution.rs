//! Texts of the depot commands: `search distributions`, `install
//! distribution|module`, `pin`/`unpin module`, `rollback`, `status
//! distribution`.

pub mod usage {
    pub const SEARCH: &str =
        "Use: search distributions <query> [--channel beta|stable|prod | --all]";
    pub const SEARCH_MODULES: &str = "Use: search modules <name> [range]";
    pub const INSTALL_DISTRIBUTION: &str =
        "Use: install distribution <product>-<version> | <product> [<range>] [--yes]";
    pub const INSTALL_MODULE: &str = "Use: install module <name> <version|range> [--yes]";
    pub const PIN: &str = "Use: pin module <name> <version>";
    pub const UNPIN: &str = "Use: unpin module <name>";
    pub const ROLLBACK: &str = "Use: rollback [--yes]";
    pub const STATUS: &str = "Use: status distribution";
}

pub mod commands {
    pub const PIN_DESCRIPTION: &str = "Keep a module as installed when distributions change";
    pub const PIN_SYNOPSIS: &str = "pin module <name> <version>";
    pub const PIN_DETAILS: &[&str] = &[
        "pin module <name> <version> – install distribution keeps this module as is",
        "the version must be the installed one; the plan shows pinned modules",
    ];
    pub const UNPIN_DESCRIPTION: &str = "Release a pinned module";
    pub const UNPIN_SYNOPSIS: &str = "unpin module <name>";
    pub const UNPIN_DETAILS: &[&str] =
        &["unpin module <name> – the next install distribution updates it again"];
    pub const ROLLBACK_DESCRIPTION: &str = "Return to the previously installed distribution";
    pub const ROLLBACK_SYNOPSIS: &str = "rollback [--yes]";
    pub const ROLLBACK_DETAILS: &[&str] = &[
        "rollback – restores the modules from before the last install distribution",
        "from the local backup if present, otherwise downloads the previous distribution again",
    ];
}

pub mod depot {
    pub const NOT_CONFIGURED: &str =
        "No depot configured: set FENRIR_DEPOT_URL, FENRIR_DEPOT_TRUSTED_KEYS and the client certificate (FENRIR_DEPOT_CLIENT_CERT/_KEY).";
    pub const NOT_BUILT: &str =
        "This Fenrir was built without the `depot` feature; depot commands are not available.";
    pub const NOTHING_CHANGED: &str = "Nothing has changed on this host.";
    pub const ABORTED: &str = "Aborted; nothing was changed.";
    pub fn error(context: &str, message: &str, code: &str) -> String {
        format!("{context}: {message} [{code}]")
    }
}

pub mod plan {
    pub const HEADERS: &[&str] = &["Module", "Rule", "Installed", "Target", "Size", "Action"];
    pub const NONE: &str = "-";
    pub fn title(title: &str) -> String {
        format!("Plan: {title}")
    }
    pub fn signed(key: &str, channel: &str, digest: &str) -> String {
        format!("  signed by key {key} · channel {channel} · manifest sha256 {digest}")
    }
    pub fn installed_now(current: &str) -> String {
        format!("  installed now: {current}")
    }
    pub fn restarts(list: &str) -> String {
        format!("  Restarts:   {list}")
    }
    pub fn migrations(list: &str) -> String {
        format!(
            "  Migrations: {list} (applied before the modules start; not reverted by a rollback)"
        )
    }
    pub fn host(include: &str, fenrir: &str) -> String {
        format!("  Host:       {include} included (this host runs fenrir {fenrir}; the host itself is not installed here)")
    }
    pub fn skipped(line: &str) -> String {
        format!("  Skipped:    {line}")
    }
    pub fn download(count: usize, size: &str, downloaded: &str) -> String {
        format!("  Bundles:    {count}, {size} ({downloaded} downloaded now), every one verified; nothing has changed yet")
    }
    pub const PROBLEMS: &str = "Cannot install, nothing was changed:";
    pub fn up_to_date(title: &str) -> String {
        format!("{title}: every module is already as planned.")
    }
    pub fn confirm(title: &str) -> String {
        format!("? Install {title} now (backup, install, health check, automatic rollback)? [Y/n] ")
    }
    pub fn confirm_rollback(title: &str) -> String {
        format!("? Run {title} now? [Y/n] ")
    }
}

pub mod run {
    pub fn done(title: &str, count: usize) -> String {
        format!("[ \x1b[38;5;76mDONE\x1b[0m ] {title}: {count} module(s) changed.")
    }
    pub fn backup(name: &str) -> String {
        format!("  backup kept for `rollback`: {name}")
    }
    pub const FAILED_CONTEXT: &str = "Install failed";
}

pub mod status {
    pub const TITLE: &str = "Distribution status";
    pub const HEADERS: &[&str] = &["Module", "Running", "Distribution", "State"];
    pub const NOTHING: &str =
        "  Installed     nothing from the depot yet (install distribution <product>-<version>)";
    pub fn installed(id: &str, channel: &str, at: &str) -> String {
        format!("  Installed     {id} (channel {channel}, installed {at})")
    }
    pub fn manifest(digest: &str, key: &str) -> String {
        format!("  Manifest      sha256 {digest} · key {key}")
    }
    pub fn follows(channel: &str, url: &str) -> String {
        format!("  Follows       {channel} (depot {url})")
    }
    pub fn overrides(list: &str) -> String {
        format!("  Overrides     {list}")
    }
    pub fn pins(list: &str) -> String {
        format!("  Pins          {list}")
    }
    pub fn previous(id: &str, how: &str) -> String {
        format!("  Previous      {id} (rollback: {how})")
    }
    pub fn newer(id: &str, channel: &str) -> String {
        format!("  Newer         {id} ({channel}) → install distribution {id}")
    }
    pub const UP_TO_DATE: &str = "  Newer         none in the followed channel";
    pub fn depot_unreachable(error: &str) -> String {
        format!("  Newer         unknown, the depot did not answer: {error}")
    }
    pub fn yanked(reason: &str) -> String {
        format!(
            "  \x1b[38;5;214mWARNING\x1b[0m       the installed distribution is yanked: {reason}"
        )
    }
}

pub mod search {
    pub const DIST_HEADERS: &[&str] = &[
        "Distribution",
        "Channel",
        "Published",
        "Modules",
        "Size",
        "Note",
    ];
    pub const MODULE_HEADERS: &[&str] =
        &["Module", "Version", "Platform", "Size", "Used in", "Note"];
    pub fn nothing(query: &str) -> String {
        format!("Nothing found for `{query}`.")
    }
    pub fn filtered(channel: &str) -> String {
        format!("(channel {channel} and above; --all shows every channel; signatures are checked at install)")
    }
}

pub mod pins {
    pub fn pinned(module: &str, version: &str) -> String {
        format!("{module} pinned at {version}: install distribution keeps it as is.")
    }
    pub fn unpinned(module: &str) -> String {
        format!("{module} unpinned: the next install distribution updates it.")
    }
    pub fn not_pinned(module: &str) -> String {
        format!("{module} is not pinned.")
    }
    pub fn not_installed(module: &str) -> String {
        format!("{module} is not installed; only an installed version can be pinned.")
    }
    pub fn other_version(module: &str, installed: &str, wanted: &str) -> String {
        format!(
            "{module} {installed} is installed, not {wanted}; run `install module {module} {wanted}` first or pin {installed}."
        )
    }
}
