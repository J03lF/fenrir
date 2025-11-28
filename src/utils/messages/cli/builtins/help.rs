pub mod command {
    pub const NAME: &str = "help";
    pub const DESCRIPTION: &str = "Lists commands or shows details for a command";
    pub const USAGE: &str = "help [command]";
    pub const DETAILS: &[&str] = &[
        "help            – lists all commands",
        "help <command>  – shows details, usage, and options",
    ];
}

pub mod handler {
    pub const TABLE_HEADERS: &[&str] = &["Command", "Usage", "Description"];
    pub const COMMANDS_HEADER: &str = "Available commands:";
    pub const DETAILS_HEADER: &str = "Details:";
    pub const DETAILS_HINT: &str = "Use 'help <command>' for details";

    pub fn describe_command(name: &str, description: &str) -> String {
        format!("{name} - {description}")
    }

    pub fn usage_line(usage: &str) -> String {
        format!("Usage: {usage}")
    }

    pub fn aliases_line(aliases: &[String]) -> String {
        format!("Aliase: {}", aliases.join(", "))
    }

    pub fn detail_line(line: &str) -> String {
        format!("  - {line}")
    }

    pub fn unknown_command(name: &str) -> String {
        format!("unknown command: {name}")
    }
}
