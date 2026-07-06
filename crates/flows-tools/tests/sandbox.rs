//! Ports of the behavior in `tests/test_tools_sandbox.py`.

use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Output};

use flows_tools::sandbox::{
    format_output, run_confined, sandbox_available, testing::run_confined_inner_for_test,
    wrap_command,
};
use tempfile::tempdir;

fn arg_after<'a>(argv: &'a [String], flag: &str, n: usize) -> &'a [String] {
    let i = argv.iter().position(|a| a == flag).expect("flag present");
    &argv[i + 1..i + 1 + n]
}

fn output(code: i32, stdout: &[u8], stderr: &[u8]) -> Output {
    Output {
        status: ExitStatus::from_raw(code << 8), // unix: exit code in high byte
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
    }
}

// -- wrap_command ------------------------------------------------------------

#[test]
#[cfg(target_os = "linux")]
fn wrap_command_confines_to_repo() {
    let d = tempdir().unwrap();
    let root = d
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let argv = wrap_command(d.path(), "echo hi");
    assert_eq!(argv[0], "bwrap");
    assert_eq!(argv.iter().filter(|a| *a == "--ro-bind").count(), 1);
    assert_eq!(
        arg_after(&argv, "--ro-bind", 2),
        &["/".to_string(), "/".to_string()]
    );
    assert_eq!(arg_after(&argv, "--bind", 2), &[root.clone(), root.clone()]);
    assert_eq!(arg_after(&argv, "--chdir", 1), &[root]);
    assert_eq!(arg_after(&argv, "--tmpfs", 1), &["/tmp".to_string()]);
    assert!(argv.iter().any(|a| a == "--unshare-user"));
    assert_eq!(
        &argv[argv.len() - 3..],
        &[
            "/bin/sh".to_string(),
            "-c".to_string(),
            "echo hi".to_string()
        ]
    );
}

#[test]
#[cfg(target_os = "linux")]
fn wrap_command_clears_env_and_sets_minimal() {
    let d = tempdir().unwrap();
    let argv = wrap_command(d.path(), "printenv");
    // host env is cleared, then only a minimal safe set injected
    assert!(argv.iter().any(|a| a == "--clearenv"));
    assert!(argv
        .windows(2)
        .any(|w| w[0] == "--setenv" && w[1] == "HOME"));
    assert!(argv.windows(3).any(|w| w == ["--setenv", "HOME", "/tmp"]));
    // clearenv must come before the setenvs so it doesn't wipe them
    let clear = argv.iter().position(|a| a == "--clearenv").unwrap();
    let first_set = argv.iter().position(|a| a == "--setenv").unwrap();
    assert!(clear < first_set);
}

// -- format_output -----------------------------------------------------------

#[test]
fn format_success() {
    assert_eq!(
        format_output(&output(0, b"hello\n", b""), 20_000),
        "hello\n"
    );
}

#[test]
fn format_no_output() {
    assert_eq!(format_output(&output(0, b"", b""), 20_000), "(no output)");
}

#[test]
fn format_exit_code_appended() {
    let s = format_output(&output(2, b"", b""), 20_000);
    assert!(s.contains("[exit code: 2]"));
}

#[test]
fn format_stderr_included() {
    let s = format_output(&output(0, b"out", b"boom"), 20_000);
    assert!(s.contains("out"));
    assert!(s.contains("STDERR:\nboom"));
}

#[test]
fn format_truncates() {
    let big = vec![b'x'; 30_000];
    let s = format_output(&output(0, &big, b""), 100);
    assert!(s.len() <= 200);
}

// -- run_confined fail-closed / injected exec --------------------------------

#[test]
fn fails_closed_without_sandbox() {
    let d = tempdir().unwrap();
    let err = run_confined_inner_for_test("echo hi", d.path(), 20_000, false, |_| {
        panic!("exec must not run when unavailable")
    })
    .unwrap_err();
    assert!(err.0.contains("sandbox unavailable"));
}

#[test]
#[cfg(target_os = "linux")]
fn injected_exec_output_is_formatted() {
    let d = tempdir().unwrap();
    let out = run_confined_inner_for_test("echo hi", d.path(), 20_000, true, |argv| {
        assert_eq!(argv[0], "bwrap"); // it wrapped the command
        Ok(output(0, b"wrapped-ok", b""))
    })
    .unwrap();
    assert_eq!(out, "wrapped-ok");
}

// -- real containment (skips where bwrap userns is unavailable) --------------

#[test]
fn real_sandbox_confines_writes_to_repo() {
    if !sandbox_available() {
        eprintln!("SKIP real_sandbox_confines_writes_to_repo: functional bwrap not available");
        return;
    }
    let d = tempdir().unwrap();
    let repo = d.path().join("repo");
    std::fs::create_dir(&repo).unwrap();

    run_confined("echo hi > inside.txt", &repo, 30, 20_000).unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join("inside.txt"))
            .unwrap()
            .trim(),
        "hi"
    );

    let outside = d.path().join("outside_pwn.txt");
    let _ = run_confined(
        &format!("echo x > {}", outside.display()),
        &repo,
        30,
        20_000,
    );
    assert!(!outside.exists(), "write outside repo should be blocked");

    let victim = d.path().join("victim.txt");
    std::fs::write(&victim, "keep").unwrap();
    let _ = run_confined(&format!("rm -f {}", victim.display()), &repo, 30, 20_000);
    assert!(victim.exists(), "delete outside repo should be blocked");
}
