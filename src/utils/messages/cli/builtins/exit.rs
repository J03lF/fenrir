pub mod command {
    pub const NAME: &str = "exit";
    pub const DESCRIPTION: &str = "Ends the current session";
    pub const USAGE: &str = "exit";
    pub const DETAILS: &[&str] = &[
        "exit – closes the current CLI/SSH session",
        "In subshells (e.g. db-shell) 'exit' returns to the main shell",
    ];
}

pub mod handler {
    pub fn shutting_down() -> &'static str {
        "Ending session ..."
    }
}
