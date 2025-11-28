pub mod command {
    pub const NAME: &str = "exit";
    pub const DESCRIPTION: &str = "Beendet die aktuelle Sitzung";
    pub const USAGE: &str = "exit";
    pub const DETAILS: &[&str] = &[
        "exit – beendet die aktuelle CLI/SSH-Sitzung",
        "In Subshells (z. B. db-shell) kehrt 'exit' zur Hauptshell zurück",
    ];
}

pub mod handler {
    pub fn shutting_down() -> &'static str {
        "Sitzung wird beendet ..."
    }
}
