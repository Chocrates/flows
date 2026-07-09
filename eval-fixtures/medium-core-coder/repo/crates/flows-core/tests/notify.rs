use flows_core::notify::notify_command;

#[cfg(target_os = "linux")]
#[test]
fn linux_uses_notify_send() {
    let (cmd, args) = notify_command("flows", "done", false).unwrap();
    assert_eq!(cmd, "notify-send");
    assert_eq!(args, vec!["flows".to_string(), "done".to_string()]);
    // sound adds a hint
    let (_, args) = notify_command("t", "b", true).unwrap();
    assert!(args.iter().any(|a| a.contains("sound-name")));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_uses_osascript() {
    let (cmd, args) = notify_command("flows", "done", false).unwrap();
    assert_eq!(cmd, "osascript");
    assert_eq!(args[0], "-e");
    assert!(args[1].contains("display notification"));
}

#[test]
fn notify_never_panics() {
    // best-effort: even if the notifier is missing, this must not fail
    flows_core::notify::notify("flows-test", "body", false);
}

#[test]
fn notify_command_includes_sound_when_enabled() {
    let (_cmd, args) = flows_core::notify::notify_command("t", "b", true).unwrap();
    // linux: notify-send gets a sound hint; macos: osascript script mentions sound
    let joined = args.join(" ");
    assert!(joined.contains("sound"));
}

#[test]
fn notify_command_no_sound_by_default() {
    let (_cmd, args) = flows_core::notify::notify_command("t", "b", false).unwrap();
    assert!(!args.join(" ").contains("sound"));
}
