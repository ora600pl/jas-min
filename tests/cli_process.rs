use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn isolated_workdir(test_name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "jas-min-{test_name}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn no_input_exits_with_usage_error_without_creating_an_artifact() {
    let workdir = isolated_workdir("no-input");
    let output = Command::new(env!("CARGO_BIN_EXE_jas-min"))
        .current_dir(&workdir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(!workdir.join("report_for_ai.toon").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no input supplied"));

    fs::remove_dir_all(workdir).unwrap();
}

#[test]
fn missing_directory_exits_with_usage_error_without_creating_an_artifact() {
    let workdir = isolated_workdir("missing-directory");
    let output = Command::new(env!("CARGO_BIN_EXE_jas-min"))
        .current_dir(&workdir)
        .args(["--directory", "missing", "--quiet"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(!workdir.join("report_for_ai.toon").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("is not a directory"));

    fs::remove_dir_all(workdir).unwrap();
}

#[test]
fn version_is_available_without_loading_the_environment() {
    let workdir = isolated_workdir("version");
    fs::write(workdir.join(".env"), "this is not valid dotenv syntax\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jas-min"))
        .current_dir(&workdir)
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("jas-min 0.9.3"));
    assert!(output.stderr.is_empty());

    fs::remove_dir_all(workdir).unwrap();
}

#[test]
fn help_describes_automatic_elastic_net_lambda_without_a_fixed_default() {
    let workdir = isolated_workdir("elastic-net-help");
    let output = Command::new(env!("CARGO_BIN_EXE_jas-min"))
        .current_dir(&workdir)
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("When omitted, lambda is selected automatically"));
    assert!(help.contains("alpha = 0.0 -> Ridge-like (pure L2) [default: 0.2]"));
    assert!(!help.contains("Elastic Net regularization [default: 30]"));

    fs::remove_dir_all(workdir).unwrap();
}

#[test]
fn invalid_elastic_net_configuration_exits_before_parsing_the_report() {
    let workdir = isolated_workdir("invalid-elastic-net");
    fs::write(workdir.join("report.html"), "not an AWR report").unwrap();

    let negative_lambda = Command::new(env!("CARGO_BIN_EXE_jas-min"))
        .current_dir(&workdir)
        .args(["--file", "report.html", "--en-lambda=-1", "--quiet"])
        .output()
        .unwrap();
    assert_eq!(negative_lambda.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&negative_lambda.stderr)
        .contains("--en-lambda must be a finite value >= 0"));

    let automatic_pure_l2 = Command::new(env!("CARGO_BIN_EXE_jas-min"))
        .current_dir(&workdir)
        .args(["--file", "report.html", "--en-alpha", "0", "--quiet"])
        .output()
        .unwrap();
    assert_eq!(automatic_pure_l2.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&automatic_pure_l2.stderr)
        .contains("automatic Elastic Net lambda selection requires --en-alpha > 0"));
    assert!(!workdir.join("report_for_ai.toon").exists());

    fs::remove_dir_all(workdir).unwrap();
}
