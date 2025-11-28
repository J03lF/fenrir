pub mod command {
    pub const NAME: &str = "show";
    pub const DESCRIPTION: &str = "Zeigt Modul-Metadaten";
    pub const USAGE: &str = "show module <name[@version]>";
    pub const DETAILS: &[&str] = &["show module <name[@version]> – zeigt Modul-Metadaten"];
    pub const RESOURCE_COMPLETIONS: &[&str] = &["module", "modules"];
}

pub mod handler {
    pub const USAGE_GENERIC: &str = "Nutzung: show <user|ticket|module> <ziel>";
    pub const USAGE_MODULE: &str = "Nutzung: show module <name[@version]>";
    pub const VALID_RESOURCES: &str = "verfügbar: show module";

    pub fn unknown_resource(resource: &str) -> String {
        format!("unbekannte Ressource: {resource}")
    }
}
