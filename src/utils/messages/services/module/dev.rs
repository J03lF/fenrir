use std::fmt;

pub mod logs {
    pub const ACTIVATING_DEV_OVERRIDE: &str = "activating module dev service override";
    pub const PACKAGING_FROM_DEV_SOURCES: &str = "packaging module from dev sources";
    pub const REGISTER_DECLARED_SERVICES_FAILED: &str =
        "failed to register declared services after sync";
    pub const STOP_RUNTIME_FOR_DEV_FAILED: &str =
        "failed to stop module runtime before activating dev services";
}

pub mod errors {
    use super::fmt;

    pub fn stop_all_failed(err: impl fmt::Display) -> String {
        format!("Module konnten nicht gestoppt werden: {err}")
    }

    pub fn install_path_missing(path: impl fmt::Display) -> String {
        format!("Installationspfad {path} existiert nicht")
    }

    pub fn no_dev_services(module_id: impl fmt::Display) -> String {
        format!("Keine Dev-Services für Modul {module_id} konfiguriert")
    }

    pub fn dev_root_not_directory(root: impl fmt::Display, module: impl fmt::Display) -> String {
        format!(
            "Dev-Verzeichnis {root} für Modul {module} ist kein Ordner",
            root = root,
            module = module
        )
    }

    pub fn missing_build_dir(root: impl fmt::Display) -> String {
        format!(
            "Dev-Verzeichnis {root} gefunden, aber kein Build-Ordner (dist/, build/, target/*) vorhanden. Lege eine `.fenrir-dev.toml` mit `output = \"pfad\"` an.",
            root = root
        )
    }

    pub fn directory_missing(path: impl fmt::Display) -> String {
        format!("Verzeichnis {path} existiert nicht")
    }

    pub fn not_a_directory(path: impl fmt::Display) -> String {
        format!("{path} ist kein Verzeichnis")
    }

    pub fn dev_config_read_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("Dev-Konfiguration {path} konnte nicht gelesen werden: {err}")
    }

    pub fn dev_config_invalid(path: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("Dev-Konfiguration {path} ist ungültig: {err}")
    }

    pub fn dev_output_empty(path: impl fmt::Display) -> String {
        format!(
            "Dev-Konfiguration {path} enthält einen leeren Output-Pfad",
            path = path
        )
    }

    pub fn dev_output_missing(path: impl fmt::Display, target: impl fmt::Display) -> String {
        format!("Dev-Konfiguration {path} verweist auf nicht existierendes Verzeichnis {target}")
    }

    pub fn dev_output_not_dir(path: impl fmt::Display, target: impl fmt::Display) -> String {
        format!("Dev-Konfiguration {path} verweist auf {target} (kein Verzeichnis)")
    }

    pub fn dev_service_missing_id(path: impl fmt::Display) -> String {
        format!(
            "Dev-Konfiguration {path} enthält einen Service ohne id",
            path = path
        )
    }

    pub fn dev_service_missing_endpoint(path: impl fmt::Display, id: impl fmt::Display) -> String {
        format!(
            "Dev-Service {id} in {path} benötigt einen Endpoint",
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
        format!("Dev-Service {id} in {path} besitzt einen ungültigen Endpoint {endpoint}: {err}")
    }
}

pub mod notes {
    use super::fmt;

    pub fn dev_endpoint(endpoint: impl fmt::Display) -> String {
        format!("dev endpoint {endpoint}")
    }

    pub const DEV_OVERRIDE_ACTIVE: &str = "Dev-Service override aktiv";

    pub fn endpoint(endpoint: impl fmt::Display) -> String {
        format!("endpoint {endpoint}")
    }
}

pub mod names {
    use super::fmt;

    pub fn binding(service_id: impl fmt::Display, module_id: impl fmt::Display) -> String {
        format!("Service {service_id} aus Modul {module_id}")
    }

    pub fn default_dev_service() -> &'static str {
        "Dev-Service"
    }

    pub fn fallback_service_name(
        module_id: impl fmt::Display,
        binding_id: impl fmt::Display,
    ) -> String {
        format!("{module_id} ({binding_id})")
    }
}
