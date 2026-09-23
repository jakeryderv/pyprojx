use std::process::Command;

fn pyprojx() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pyprojx"))
}

#[test]
fn version_matches_package() {
    let output = pyprojx().arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("pyprojx {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_describes_the_tool() {
    let output = pyprojx().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Version-aware intelligence for pyproject.toml"));
}

#[test]
fn unknown_argument_is_a_usage_error() {
    let output = pyprojx().arg("--no-such-flag").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn no_arguments_prints_help_as_usage_error() {
    let output = pyprojx().output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("Usage: pyprojx")
    );
}
