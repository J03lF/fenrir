pub mod action_kind {
    pub const START: &str = "start";
    pub const STOP: &str = "stop";
    pub const RESTART: &str = "restart";
}

pub mod kind {
    pub const INFRASTRUCTURE: &str = "infra";
    pub const TRANSPORT: &str = "transport";
    pub const BACKGROUND_JOB: &str = "job";
    pub const CLI: &str = "cli";
    pub const SECURITY: &str = "security";
    pub const STORAGE: &str = "storage";
    pub const OTHER: &str = "other";
}

pub mod tag {
    pub const CORE: &str = "core";
    pub const PLATFORM: &str = "platform";
    pub const AUXILIARY: &str = "auxiliary";
}

pub mod status {
    pub const STARTING: &str = "starting";
    pub const ACTIVE: &str = "active";
    pub const DEGRADED: &str = "degraded";
    pub const FAILED: &str = "failed";
    pub const STANDBY: &str = "standby";
    pub const STOPPED: &str = "stopped";
}
