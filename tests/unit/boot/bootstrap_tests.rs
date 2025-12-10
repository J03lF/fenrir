use super::parse_bootstrap_spec;
use crate::domain::module::ModuleId;

#[test]
fn parses_module_without_version() {
    let expected_id = ModuleId::new("fenrir-api").unwrap();
    let (id, version) = parse_bootstrap_spec("fenrir-api").expect("spec parses");

    assert_eq!(id, expected_id);
    assert!(version.is_none());
}

#[test]
fn parses_module_with_version() {
    let (id, version) = parse_bootstrap_spec("fenrir-api@1.2.3").expect("spec parses");

    assert_eq!(id, ModuleId::new("fenrir-api").unwrap());
    let version = version.expect("version present");
    assert_eq!(version.to_string(), "1.2.3");
}

#[test]
fn rejects_missing_version_segment() {
    assert!(parse_bootstrap_spec("fenrir-api@").is_err());
}

#[test]
fn rejects_invalid_version() {
    assert!(parse_bootstrap_spec("fenrir-api@not-a-version").is_err());
}
