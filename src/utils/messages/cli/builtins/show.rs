pub mod command {
    pub const NAME: &str = "show";
    pub const DESCRIPTION: &str = "Shows module metadata";
    pub const USAGE: &str = "show module <name[@version]>";
    pub const DETAILS: &[&str] = &["show module <name[@version]> – displays module metadata"];
    pub const RESOURCE_COMPLETIONS: &[&str] = &["module", "modules"];
}

pub mod handler {
    pub const USAGE_GENERIC: &str = "Usage: show <user|ticket|module> <target>";
    pub const USAGE_MODULE: &str = "Usage: show module <name[@version]>";
    pub const VALID_RESOURCES: &str = "available: show module";

    pub fn unknown_resource(resource: &str) -> String {
        format!("unknown resource: {resource}")
    }
}
