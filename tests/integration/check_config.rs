use assert_cmd::prelude::*;
use std::process::Command;

#[test]
fn check_config_exits_zero_on_valid_config() {
    let mut cmd = Command::cargo_bin("fenrir").unwrap();
    cmd.arg("--check-config");
    cmd.assert().success();
}

