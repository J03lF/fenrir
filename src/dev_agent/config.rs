use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DevAgentConfig {
    pub module_id: String,
    pub command: Vec<String>,
    pub command_display: String,
    pub workdir: PathBuf,
    pub auto_restart: bool,
    pub env: HashMap<String, String>,
    pub log_path: PathBuf,
    pub services: Vec<DevAgentService>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DevAgentService {
    pub id: String,
    pub endpoint: String,
}
