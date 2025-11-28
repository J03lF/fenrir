pub mod command {
    pub const NAME: &str = "clear";
    pub const DESCRIPTION: &str = "Clears the screen and resets the cursor";
    pub const USAGE: &str = "clear";
    pub const DETAILS: &[&str] = &[
        "clear – removes the visible screen contents",
        "Alias for the classic terminal command, useful in SSH/CLI sessions",
    ];
}
