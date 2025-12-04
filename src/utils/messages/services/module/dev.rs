use std::fmt;

pub mod logs {
    pub const ACTIVATING_DEV_OVERRIDE: &str = "activating module dev service override";
    pub const PACKAGING_FROM_DEV_SOURCES: &str = "packaging module from dev sources";
    pub const REGISTER_DECLARED_SERVICES_FAILED: &str =
        "failed to register declared services after sync";
    pub const STOP_RUNTIME_FOR_DEV_FAILED: &str =
        "failed to stop module runtime before activating dev services";
    pub const DEV_AGENT_STARTING: &str = "spawning dev agent for local override";
    pub const DEV_AGENT_STOPPING: &str = "stopping dev agent";
    pub const DEV_AGENT_SPAWN_FAILED: &str = "failed to start dev agent";
}

pub mod errors {
    use super::fmt;

    pub fn install_path_missing(path: impl fmt::Display) -> String {
        format!("installation path {path} does not exist")
    }

    pub fn no_dev_services(module_id: impl fmt::Display) -> String {
        format!("no dev services configured for module {module_id}")
    }

    pub fn dev_root_not_directory(root: impl fmt::Display, module: impl fmt::Display) -> String {
        format!(
            "dev directory {root} for module {module} is not a folder",
            root = root,
            module = module
        )
    }

    pub fn missing_build_dir(root: impl fmt::Display) -> String {
        format!(
            "dev directory {root} found, but no build folder (dist/, build/, target/*) present. Create a `.fenrir-dev.toml` with `output = \"path\"`.",
            root = root
        )
    }

    pub fn directory_missing(path: impl fmt::Display) -> String {
        format!("directory {path} does not exist")
    }

    pub fn not_a_directory(path: impl fmt::Display) -> String {
        format!("{path} is not a directory")
    }

    pub fn dev_config_read_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("failed to read dev configuration {path}: {err}")
    }

    pub fn dev_config_invalid(path: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("dev configuration {path} is invalid: {err}")
    }

    pub fn dev_output_empty(path: impl fmt::Display) -> String {
        format!(
            "dev configuration {path} contains an empty output path",
            path = path
        )
    }

    pub fn dev_output_missing(path: impl fmt::Display, target: impl fmt::Display) -> String {
        format!("dev configuration {path} points to non-existent directory {target}")
    }

    pub fn dev_output_not_dir(path: impl fmt::Display, target: impl fmt::Display) -> String {
        format!("dev configuration {path} references {target} (not a directory)")
    }

    pub fn dev_service_missing_id(path: impl fmt::Display) -> String {
        format!(
            "dev configuration {path} contains a service without an id",
            path = path
        )
    }

    pub fn dev_service_missing_endpoint(path: impl fmt::Display, id: impl fmt::Display) -> String {
        format!(
            "dev service {id} in {path} requires an endpoint",
            id = id,
            path = path
        )
    }

    pub fn dev_service_invalid_endpoint(
        path: impl fmt::Display,
        id: impl fmt::Display,
        endpoint: impl fmt::Display,
        err: impl fmt::Display,
    ) -> String {
        format!("dev service {id} in {path} has an invalid endpoint {endpoint}: {err}")
    }

    pub fn dev_service_invalid_role(
        path: impl fmt::Display,
        id: impl fmt::Display,
        role: impl fmt::Display,
    ) -> String {
        format!("dev service {id} in {path} has an unknown service role {role}")
    }

    pub fn dev_service_invalid_scope(
        path: impl fmt::Display,
        id: impl fmt::Display,
        scope: impl fmt::Display,
        err: impl fmt::Display,
    ) -> String {
        format!("dev service {id} in {path} has an invalid scope {scope}: {err}")
    }

    pub fn dev_service_invalid_rate_limit(
        path: impl fmt::Display,
        id: impl fmt::Display,
        value: impl fmt::Display,
    ) -> String {
        format!("dev service {id} in {path} has an invalid rate_limit_per_second value {value}")
    }

    pub fn start_after_sync_failed(err: impl fmt::Display) -> String {
        format!("failed to restart module after sync: {err}")
    }

    pub fn dev_run_missing_command(path: impl fmt::Display) -> String {
        format!("dev run configuration {path} requires a 'command'")
    }

    pub fn dev_run_invalid_command(path: impl fmt::Display) -> String {
        format!("dev run configuration {path} must provide at least one command argument")
    }

    pub fn dev_run_workdir_missing(path: impl fmt::Display, dir: impl fmt::Display) -> String {
        format!("dev run configuration {path} references missing workdir {dir}")
    }

    pub fn dev_agent_binary_missing(path: impl fmt::Display) -> String {
        format!("unable to locate fenrir binary near {path}; dev agent cannot start")
    }

    pub fn dev_agent_spawn_failed(module: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("failed to spawn dev agent for module {module}: {err}")
    }

    pub fn dev_agent_missing(module: impl fmt::Display) -> String {
        format!("dev agent for module {module} is not active")
    }
}

pub mod notes {
    use super::fmt;

    pub fn dev_endpoint(endpoint: impl fmt::Display) -> String {
        format!("dev endpoint {endpoint}")
    }

    pub const DEV_OVERRIDE_ACTIVE: &str = "dev-service override active";

    pub fn endpoint(endpoint: impl fmt::Display) -> String {
        format!("endpoint {endpoint}")
    }
}

pub mod names {
    use super::fmt;

    pub fn binding(service_id: impl fmt::Display, module_id: impl fmt::Display) -> String {
        format!("service {service_id} from module {module_id}")
    }

    pub fn default_dev_service() -> &'static str {
        "Dev Service"
    }

    pub fn fallback_service_name(
        module_id: impl fmt::Display,
        binding_id: impl fmt::Display,
    ) -> String {
        format!("{module_id} ({binding_id})")
    }
}
