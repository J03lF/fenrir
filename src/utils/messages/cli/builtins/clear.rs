pub mod command {
    pub const NAME: &str = "clear";
    pub const DESCRIPTION: &str = "Leert den Bildschirm und setzt den Cursor zurück";
    pub const USAGE: &str = "clear";
    pub const DETAILS: &[&str] = &[
        "clear – löscht den sichtbaren Bildschirminhalt",
        "Alias für das klassische Terminal-Kommando, nützlich in SSH/CLI-Sitzungen",
    ];
}
