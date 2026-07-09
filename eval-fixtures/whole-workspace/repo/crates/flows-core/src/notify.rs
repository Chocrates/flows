//! Desktop notifications for backgrounded runs. Port of `flows/notify.py` —
//! shells out to the platform notifier (best-effort, never fails the run).

/// Build the notifier command for this platform, or None if unsupported.
/// Pure, so the per-platform argv is testable.
pub fn notify_command(title: &str, body: &str, sound: bool) -> Option<(String, Vec<String>)> {
    if cfg!(target_os = "macos") {
        let mut script = format!("display notification {body:?} with title {title:?}");
        if sound {
            script.push_str(" sound name \"default\"");
        }
        Some(("osascript".into(), vec!["-e".into(), script]))
    } else if cfg!(target_os = "linux") {
        let mut args = vec![title.to_string(), body.to_string()];
        if sound {
            args.push("--hint=string:sound-name:message-new-instant".into());
        }
        Some(("notify-send".into(), args))
    } else {
        None
    }
}

/// Fire a desktop notification (best-effort; errors are swallowed).
pub fn notify(title: &str, body: &str, sound: bool) {
    if let Some((cmd, args)) = notify_command(title, body, sound) {
        let _ = std::process::Command::new(cmd)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}
