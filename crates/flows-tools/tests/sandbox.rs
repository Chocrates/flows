//! Ports of the behavior in `tests/test_tools_sandbox.py`.

use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Output};

use flows_tools::sandbox::{
    format_output, run_confined, sandbox_available, testing::run_confined_inner_for_test,
    wrap_command, Confinement, Limits,
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
    // `/` is the only read-only *root* bind (others are credential masks).
    assert_eq!(
        argv.windows(3)
            .filter(|w| w[0] == "--ro-bind" && w[1] == "/")
            .count(),
        1
    );
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

    run_confined(
        "echo hi > inside.txt",
        &repo,
        30,
        20_000,
        &Confinement::default(),
    )
    .unwrap();
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
        &Confinement::default(),
    );
    assert!(!outside.exists(), "write outside repo should be blocked");

    let victim = d.path().join("victim.txt");
    std::fs::write(&victim, "keep").unwrap();
    let _ = run_confined(
        &format!("rm -f {}", victim.display()),
        &repo,
        30,
        20_000,
        &Confinement::default(),
    );
    assert!(victim.exists(), "delete outside repo should be blocked");
}

// -- seccomp: no unix sockets inside the sandbox -----------------------------

#[cfg(target_os = "linux")]
fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("-c")
        .arg("pass")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
#[cfg(target_os = "linux")]
fn wrap_command_applies_seccomp() {
    let d = tempdir().unwrap();
    let argv = wrap_command(d.path(), "true");
    if flows_tools::sandbox::seccomp::SUPPORTED {
        assert_eq!(arg_after(&argv, "--seccomp", 1), ["999"]);
    }
}

#[test]
#[cfg(target_os = "linux")]
fn real_sandbox_blocks_unix_socket_connect() {
    if !sandbox_available() || !python3_available() {
        eprintln!("SKIP real_sandbox_blocks_unix_socket_connect: needs bwrap + python3");
        return;
    }
    let d = tempdir().unwrap();
    // A listening socket the host can reach — the sandbox must not.
    let _listener = std::os::unix::net::UnixListener::bind(d.path().join("host.sock")).unwrap();
    let out = run_confined(
        "python3 -c \"import socket; s=socket.socket(socket.AF_UNIX); s.connect('host.sock'); print('CONNECTED')\"",
        d.path(),
        30,
        10_000,
        &Confinement::default(),
    )
    .unwrap();
    assert!(!out.contains("CONNECTED"), "unix socket reachable: {out}");
    assert!(out.contains("PermissionError"), "unexpected: {out}");
}

#[test]
#[cfg(target_os = "linux")]
fn real_sandbox_keeps_socketpair_and_inet() {
    if !sandbox_available() || !python3_available() {
        eprintln!("SKIP real_sandbox_keeps_socketpair_and_inet: needs bwrap + python3");
        return;
    }
    let d = tempdir().unwrap();
    let out = run_confined(
        "python3 -c \"import socket; a,b=socket.socketpair(); a.send(b'ok'); print(b.recv(2).decode()); socket.socket(socket.AF_INET).close(); print('INET')\"",
        d.path(),
        30,
        10_000,
        &Confinement::default(),
    )
    .unwrap();
    assert!(
        out.contains("ok") && out.contains("INET"),
        "unexpected: {out}"
    );
}

// -- resource limits ---------------------------------------------------------

#[cfg(target_os = "linux")]
fn cgroup_limits() -> bool {
    sandbox_available()
        && flows_tools::sandbox::limits_mode() == flows_tools::sandbox::LimitMode::Cgroup
}

#[test]
#[cfg(target_os = "linux")]
fn real_sandbox_memory_cap_kills_hog() {
    if !cgroup_limits() || !python3_available() {
        eprintln!(
            "SKIP real_sandbox_memory_cap_kills_hog: needs bwrap + systemd user scope + python3"
        );
        return;
    }
    let d = tempdir().unwrap();
    let limits = Limits {
        memory_mb: 64,
        max_procs: 0,
    };
    let out = run_confined(
        "python3 -c \"b = bytearray(512 * 1024 * 1024); print('ALLOCATED')\"",
        d.path(),
        60,
        10_000,
        &Confinement {
            limits,
            ..Confinement::default()
        },
    )
    .unwrap();
    assert!(!out.contains("ALLOCATED"), "hog survived: {out}");
    assert!(out.contains("tools.bash.memory_mb"), "missing hint: {out}");
}

#[test]
#[cfg(target_os = "linux")]
fn real_sandbox_task_cap_contains_fork_bomb() {
    if !cgroup_limits() || !python3_available() {
        eprintln!("SKIP real_sandbox_task_cap_contains_fork_bomb: needs bwrap + systemd user scope + python3");
        return;
    }
    let d = tempdir().unwrap();
    let limits = Limits {
        memory_mb: 0,
        max_procs: 20,
    };
    // Try to start 100 sleepers; the cap stops it well short (spawns fail with
    // EAGAIN, which `sh` treats as fatal, hence python).
    let out = run_confined(
        "python3 -c \"import subprocess as s\nn=0\nfor _ in range(100):\n  try: s.Popen(['sleep','5']); n+=1\n  except OSError: pass\nprint('running=%d' % n)\"",
        d.path(),
        60,
        10_000,
        &Confinement {
            limits,
            ..Confinement::default()
        },
    )
    .unwrap();
    let running: usize = out
        .lines()
        .find_map(|l| l.strip_prefix("running="))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("unexpected: {out}"));
    assert!(
        running <= 20,
        "task cap not enforced ({running} procs): {out}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn real_sandbox_normal_command_unaffected_by_limits() {
    if !sandbox_available() {
        eprintln!("SKIP real_sandbox_normal_command_unaffected_by_limits: needs bwrap");
        return;
    }
    let d = tempdir().unwrap();
    let out = run_confined("echo ok", d.path(), 30, 1_000, &Confinement::default()).unwrap();
    assert_eq!(out.trim(), "ok");
}

// -- offline (no network) ---------------------------------------------------

#[test]
#[cfg(target_os = "linux")]
fn wrap_unshares_net_only_when_offline() {
    let d = tempdir().unwrap();
    let on = flows_tools::sandbox::wrap_command_with(d.path(), "true", &Confinement::default());
    assert!(!on.iter().any(|a| a == "--unshare-net"));
    let off = flows_tools::sandbox::wrap_command_with(
        d.path(),
        "true",
        &Confinement {
            offline: true,
            ..Confinement::default()
        },
    );
    assert!(off.iter().any(|a| a == "--unshare-net"));
}

#[test]
#[cfg(target_os = "linux")]
fn real_sandbox_offline_cuts_host_network_keeps_loopback() {
    if !sandbox_available() || !python3_available() {
        eprintln!(
            "SKIP real_sandbox_offline_cuts_host_network_keeps_loopback: needs bwrap + python3"
        );
        return;
    }
    // A host listener on 127.0.0.1 is reachable through the shared network
    // namespace by default, and not from an offline sandbox (its own loopback).
    let host = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = host.local_addr().unwrap().port();
    let probe = format!(
        "python3 -c \"import socket\ntry:\n  socket.create_connection(('127.0.0.1', {port}), 2); print('HOST-REACHED')\nexcept OSError as e: print('BLOCKED', e)\ns=socket.socket(); s.bind(('127.0.0.1', 0)); s.listen(1)\nc=socket.create_connection(s.getsockname(), 2); print('LOOPBACK-OK')\""
    );
    let d = tempdir().unwrap();
    let online = run_confined(&probe, d.path(), 30, 10_000, &Confinement::default()).unwrap();
    assert!(online.contains("HOST-REACHED"), "{online}");
    let offline = run_confined(
        &probe,
        d.path(),
        30,
        10_000,
        &Confinement {
            offline: true,
            ..Confinement::default()
        },
    )
    .unwrap();
    assert!(!offline.contains("HOST-REACHED"), "{offline}");
    assert!(offline.contains("BLOCKED"), "{offline}");
    assert!(offline.contains("LOOPBACK-OK"), "{offline}");
}

// -- credential masking ------------------------------------------------------

#[test]
#[cfg(target_os = "linux")]
fn wrap_masks_before_repo_bind() {
    let d = tempdir().unwrap();
    let home = d.path().canonicalize().unwrap();
    std::fs::create_dir(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".netrc"), "x").unwrap();
    let repo = home.join("repo");
    std::fs::create_dir(&repo).unwrap();
    let hidden = flows_tools::sandbox::hidden_paths_in(&home, false);
    let argv =
        flows_tools::sandbox::backend().wrap(&repo, "true", &Confinement::default(), &hidden);
    let ssh = home.join(".ssh").display().to_string();
    let netrc = home.join(".netrc").display().to_string();
    let tmpfs = argv.windows(2).position(|w| w == ["--tmpfs", ssh.as_str()]);
    let null = argv
        .windows(3)
        .position(|w| w == ["--ro-bind", "/dev/null", netrc.as_str()]);
    let bind = argv.iter().position(|a| a == "--bind").unwrap();
    assert!(tmpfs.unwrap() < bind && null.unwrap() < bind, "{argv:?}");
}

/// A fake home with an SSH key, a .netrc, and flows' API-key config. Not under
/// `/tmp`: the sandbox mounts its own `/tmp`, which would hide it regardless.
#[cfg(target_os = "linux")]
fn fake_home() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let d = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let home = d.path().canonicalize().unwrap();
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/id_test"), "SSH-SECRET").unwrap();
    std::fs::write(home.join(".netrc"), "NETRC-SECRET").unwrap();
    std::fs::create_dir_all(home.join(".flows")).unwrap();
    std::fs::write(home.join(".flows/config.yml"), "API-KEY-SECRET").unwrap();
    let repo = home.join("work/repo");
    std::fs::create_dir_all(&repo).unwrap();
    (d, home, repo)
}

#[test]
#[cfg(target_os = "linux")]
fn real_sandbox_hides_credentials_by_absolute_path() {
    use flows_tools::sandbox::testing::run_confined_with_home;
    if !sandbox_available() {
        eprintln!("SKIP real_sandbox_hides_credentials_by_absolute_path: needs bwrap");
        return;
    }
    let (_d, home, repo) = fake_home();
    let h = home.display();
    let cmd = format!(
        "cat {h}/.ssh/id_test {h}/.netrc {h}/.flows/config.yml; echo LS=$(ls -A {h}/.ssh)END"
    );
    let out = run_confined_with_home(&cmd, &repo, &home, false).unwrap();
    assert!(!out.contains("SECRET"), "credential leaked: {out}");
    // control: the same home is otherwise visible (not hidden by /tmp)
    let ctl = run_confined_with_home(&format!("ls {h}/work"), &repo, &home, false).unwrap();
    assert!(ctl.contains("repo"), "fake home not visible at all: {ctl}");
    assert!(out.contains("LS=END"), "masked dir not empty: {out}");
}

#[test]
#[cfg(target_os = "linux")]
fn real_sandbox_expose_credentials_keeps_flows_config_hidden() {
    use flows_tools::sandbox::testing::run_confined_with_home;
    if !sandbox_available() {
        eprintln!("SKIP real_sandbox_expose_credentials_keeps_flows_config_hidden: needs bwrap");
        return;
    }
    let (_d, home, repo) = fake_home();
    let h = home.display();
    let cmd = format!("cat {h}/.ssh/id_test; echo; cat {h}/.flows/config.yml");
    let out = run_confined_with_home(&cmd, &repo, &home, true).unwrap();
    assert!(out.contains("SSH-SECRET"), "opt-out did not expose: {out}");
    assert!(
        !out.contains("API-KEY-SECRET"),
        "flows config leaked: {out}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn real_sandbox_repo_inside_masked_dir_still_works() {
    use flows_tools::sandbox::testing::run_confined_with_home;
    if !sandbox_available() {
        eprintln!("SKIP real_sandbox_repo_inside_masked_dir_still_works: needs bwrap");
        return;
    }
    let (_d, home, _repo) = fake_home();
    let repo = home.join(".ssh/project");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("README"), "readme-here").unwrap();
    let out = run_confined_with_home(
        "cat README; echo made > new.txt; cat ../id_test",
        &repo,
        &home,
        false,
    )
    .unwrap();
    assert!(out.contains("readme-here"), "{out}");
    assert!(!out.contains("SSH-SECRET"), "sibling key leaked: {out}");
    assert_eq!(
        std::fs::read_to_string(repo.join("new.txt"))
            .unwrap()
            .trim(),
        "made"
    );
}
