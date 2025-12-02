pub mod logs {
    pub const STATE_LOAD_FAILED: &str = "failed to restore module port assignments";
    pub const STATE_SAVE_FAILED: &str = "failed to persist module port assignments";
    pub const PORT_ASSIGNED: &str = "assigned dynamic port to module";
    pub const PORT_REUSED: &str = "reusing persisted dynamic port";
    pub const PORT_RELEASED: &str = "released dynamic port assignment";
    pub const PORT_UNAVAILABLE_RECLAIM: &str =
        "previous dynamic port was not available and will be reassigned";
    pub const PORT_RANGE_EXHAUSTED: &str = "no dynamic ports available in configured range";
}
