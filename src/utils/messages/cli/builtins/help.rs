pub mod command {
    pub const NAME: &str = "help";
    pub const DESCRIPTION: &str = "Listet Befehle oder zeigt Details zu einem Befehl an";
    pub const USAGE: &str = "help [befehl]";
    pub const DETAILS: &[&str] = &[
        "help            – listet alle Befehle",
        "help <befehl>   – zeigt Details, Usage und Optionen",
    ];
}

pub mod handler {
    pub const TABLE_HEADERS: &[&str] = &["Befehl", "Usage", "Beschreibung"];
    pub const COMMANDS_HEADER: &str = "Verfügbare Befehle:";
    pub const DETAILS_HEADER: &str = "Details:";
    pub const DETAILS_HINT: &str = "Nutze 'help <befehl>' für Details";

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
        format!("unbekannter Befehl: {name}")
    }
}
