//! Template registry against real local git repositories.

use std::path::Path;
use std::process::Command;

use flows_core::registry::{self, Source};
use flows_core::templates::{list_templates, load_template};
use tempfile::tempdir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const SHIP_V1: &str = "phases:\n  - name: build\n    type: script\n    command: make build\n  - name: implement\n    tools:\n      files: true\n      bash:\n        allowed: [\"cargo*\"]\n        expose_credentials: true\n";
const SHIP_V2: &str = "phases:\n  - name: build\n    type: script\n    command: make release\n";

/// A repo with `pkgs/ship.yml` + prompts at tag v1, then a v2 commit on main.
fn source_repo() -> (tempfile::TempDir, String, String) {
    let d = tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q", "-b", "main"]);
    write(&r.join("pkgs/ship.yml"), SHIP_V1);
    write(
        &r.join("pkgs/prompts/ship/implement.md"),
        "Implement {{ idea }}\n",
    );
    write(&r.join("multi/a.yml"), "phases: []\n");
    write(&r.join("multi/b.yml"), "phases: []\n");
    git(r, &["add", "-A"]);
    git(r, &["commit", "-qm", "v1"]);
    git(r, &["tag", "v1"]);
    let v1 = git(r, &["rev-parse", "HEAD"]);
    write(&r.join("pkgs/ship.yml"), SHIP_V2);
    std::fs::remove_dir_all(r.join("pkgs/prompts")).unwrap();
    git(r, &["add", "-A"]);
    git(r, &["commit", "-qm", "v2"]);
    let v2 = git(r, &["rev-parse", "HEAD"]);
    (d, v1, v2)
}

fn src(repo: &Path, rest: &str) -> Source {
    Source::parse(&format!("{}{rest}", repo.display())).unwrap()
}

#[test]
fn install_pins_reviews_and_confines() {
    let (repo, v1, _) = source_repo();
    let flows = tempdir().unwrap();
    let s = src(repo.path(), "//pkgs@v1");
    let pkg = registry::fetch(&s).unwrap();
    assert_eq!(pkg.commit, v1);
    assert_eq!(pkg.stem, "ship");
    assert_eq!(pkg.prompts.len(), 1);

    let review = registry::review(&s, &pkg, "ship", false);
    assert!(review.contains("$ make build"), "{review}");
    assert!(review.contains("! expose_credentials: true"), "{review}");
    assert!(review.contains("Untrusted"), "{review}");

    registry::check_new_name(flows.path(), "ship").unwrap();
    registry::install(flows.path(), &s, &pkg, "ship", false).unwrap();
    let lock = registry::read_lock(flows.path());
    assert_eq!(lock.templates["ship"].commit, v1);
    assert_eq!(lock.templates["ship"].git_ref.as_deref(), Some("v1"));
    assert!(flows.path().join("prompts/ship/implement.md").is_file());
    assert!(list_templates(Some(flows.path()), None).contains(&"ship".to_string()));

    let proj = tempdir().unwrap();
    let (phases, _) = load_template(Some("ship"), proj.path(), Some(flows.path())).unwrap();
    assert!(phases.iter().all(|p| p.untrusted));
    assert!(!phases[1].tools.bash.as_ref().unwrap().expose_credentials);

    // Reinstall must go through update.
    assert!(registry::check_new_name(flows.path(), "ship")
        .unwrap_err()
        .contains("update"));

    // Trust lifts the restrictions.
    registry::set_trust(flows.path(), "ship", true).unwrap();
    let (phases, _) = load_template(Some("ship"), proj.path(), Some(flows.path())).unwrap();
    assert!(phases.iter().all(|p| !p.untrusted));
    assert!(phases[1].tools.bash.as_ref().unwrap().expose_credentials);
}

#[test]
fn default_ref_is_the_default_branch() {
    let (repo, _, v2) = source_repo();
    let pkg = registry::fetch(&src(repo.path(), "//pkgs/ship.yml")).unwrap();
    assert_eq!(pkg.commit, v2);
    assert!(pkg.template.contains("make release"));
}

#[test]
fn bad_paths_and_names_are_refused() {
    let (repo, _, _) = source_repo();
    let e = registry::fetch(&src(repo.path(), "//multi")).unwrap_err();
    assert!(e.contains("a.yml") && e.contains("b.yml"), "{e}");
    let e = registry::fetch(&src(repo.path(), "//nope")).unwrap_err();
    assert!(e.contains("not found"), "{e}");
    let e = registry::fetch(&src(repo.path(), "@no-such-ref")).unwrap_err();
    assert!(e.contains("no-such-ref"), "{e}");

    let flows = tempdir().unwrap();
    assert!(registry::check_new_name(flows.path(), "feature")
        .unwrap_err()
        .contains("builtin"));
    write(&flows.path().join("templates/mine.yml"), "phases: []\n");
    assert!(registry::check_new_name(flows.path(), "mine")
        .unwrap_err()
        .contains("--as"));
    assert!(registry::check_new_name(flows.path(), "../x").is_err());
}

#[test]
fn symlinked_template_is_refused() {
    let d = tempdir().unwrap();
    let secret = d.path().join("secret.txt");
    std::fs::write(&secret, "phases: []\n").unwrap();
    let pkg = d.path().join("pkg");
    std::fs::create_dir(&pkg).unwrap();
    std::os::unix::fs::symlink(&secret, pkg.join("t.yml")).unwrap();
    let e = registry::read_package(d.path(), "pkg/t.yml", "c".into()).unwrap_err();
    assert!(e.contains("not a regular file"), "{e}");
}

#[test]
fn update_diff_then_remove() {
    let (repo, _, _) = source_repo();
    let flows = tempdir().unwrap();
    let s = src(repo.path(), "//pkgs@v1");
    let v1 = registry::fetch(&s).unwrap();
    registry::install(flows.path(), &s, &v1, "ship", false).unwrap();

    let main = registry::fetch(&src(repo.path(), "//pkgs@main")).unwrap();
    let diff = registry::update_diff(flows.path(), "ship", &main);
    assert!(diff.contains("-    command: make build"), "{diff}");
    assert!(diff.contains("+    command: make release"), "{diff}");
    assert!(
        diff.contains("--- removed prompts/ship/implement.md"),
        "{diff}"
    );
    assert!(registry::update_diff(flows.path(), "ship", &v1).is_empty());

    registry::remove(flows.path(), "ship").unwrap();
    assert!(!flows.path().join("templates/ship.yml").exists());
    assert!(!flows.path().join("prompts/ship").exists());
    assert!(registry::read_lock(flows.path()).templates.is_empty());
    assert!(registry::remove(flows.path(), "ship").is_err());
}

#[test]
fn project_override_is_trusted_and_includes_stay_confined() {
    let (repo, _, _) = source_repo();
    let flows = tempdir().unwrap();
    let s = src(repo.path(), "//pkgs@v1");
    registry::install(
        flows.path(),
        &s,
        &registry::fetch(&s).unwrap(),
        "ship",
        false,
    )
    .unwrap();

    // A local template including the installed one: only its phases confined.
    let proj = tempdir().unwrap();
    write(
        &proj.path().join(".flows/templates/mine.yml"),
        "phases:\n  - name: plan\n  - include: ship\n",
    );
    let (phases, _) = load_template(Some("mine"), proj.path(), Some(flows.path())).unwrap();
    let flags: Vec<(&str, bool)> = phases
        .iter()
        .map(|p| (p.name.as_str(), p.untrusted))
        .collect();
    assert_eq!(
        flags,
        [("plan", false), ("build", true), ("implement", true)]
    );

    // A project template named like the installed one wins and is trusted.
    write(
        &proj.path().join(".flows/templates/ship.yml"),
        "phases:\n  - name: build\n    type: script\n    command: make\n",
    );
    let (phases, _) = load_template(Some("ship"), proj.path(), Some(flows.path())).unwrap();
    assert!(!phases[0].untrusted);
}
