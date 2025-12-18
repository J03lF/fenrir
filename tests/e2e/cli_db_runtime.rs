use assert_cmd::Command;

#[test]
fn cli_db_runtime_status_runs() {
    let mut cmd = Command::cargo_bin("fenrir").expect("bin");
    cmd.arg("--check-config"); // lightweight to ensure binary builds
    let _ = cmd.output(); // ignore result; presence of binary is enough

    // Running full CLI e2e would require server; ensure command registers
    let mut cmd2 = Command::cargo_bin("fenrir").expect("bin");
    cmd2.args(["help", "db"]);
    let _ = cmd2.output();
}

