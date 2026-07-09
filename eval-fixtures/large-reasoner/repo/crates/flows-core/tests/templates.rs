//! Ports of the behavior in `tests/test_templates.py`.

use flows_core::models::PhaseType;
use flows_core::templates::{list_templates, load_template, TemplateError};
use tempfile::tempdir;

#[test]
fn builtins_load_and_have_phases() {
    let repo = tempdir().unwrap();
    let flows = tempdir().unwrap();
    for name in ["feature", "software", "research", "review", "blank"] {
        let (phases, text) = load_template(Some(name), repo.path(), Some(flows.path())).unwrap();
        assert!(!phases.is_empty(), "{name} had no phases");
        assert!(text.contains("phases"), "{name} yaml missing phases");
    }
}

#[test]
fn default_is_feature() {
    let repo = tempdir().unwrap();
    let flows = tempdir().unwrap();
    let (def, _) = load_template(None, repo.path(), Some(flows.path())).unwrap();
    let (feat, _) = load_template(Some("feature"), repo.path(), Some(flows.path())).unwrap();
    assert_eq!(def.len(), feat.len());
    assert_eq!(def[0].name, feat[0].name);
}

#[test]
fn feature_has_expected_phase_shape() {
    let repo = tempdir().unwrap();
    let flows = tempdir().unwrap();
    let (phases, _) = load_template(Some("feature"), repo.path(), Some(flows.path())).unwrap();
    let names: Vec<_> = phases.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"plan"));
    assert!(names.contains(&"implement"));
    assert!(names.contains(&"review"));
    assert!(names.contains(&"approve"));
    // commit is a script phase
    let commit = phases.iter().find(|p| p.name == "commit").unwrap();
    assert_eq!(commit.phase_type, PhaseType::Script);
    // implement got the raised tool-round budget
    let implement = phases.iter().find(|p| p.name == "implement").unwrap();
    assert_eq!(implement.max_tool_rounds, Some(40));
}

#[test]
fn unknown_template_errors() {
    let repo = tempdir().unwrap();
    let flows = tempdir().unwrap();
    let err = load_template(Some("nope"), repo.path(), Some(flows.path())).unwrap_err();
    assert!(matches!(err, TemplateError::Unknown { .. }));
}

#[test]
fn global_override_shadows_builtin() {
    let repo = tempdir().unwrap();
    let flows = tempdir().unwrap();
    let tdir = flows.path().join("templates");
    std::fs::create_dir_all(&tdir).unwrap();
    std::fs::write(
        tdir.join("feature.yml"),
        "phases:\n  - name: only_phase\n    type: agent\n",
    )
    .unwrap();
    let (phases, _) = load_template(Some("feature"), repo.path(), Some(flows.path())).unwrap();
    assert_eq!(phases.len(), 1);
    assert_eq!(phases[0].name, "only_phase");
}

#[test]
fn project_override_shadows_global_and_builtin() {
    let repo = tempdir().unwrap();
    let flows = tempdir().unwrap();
    // global override
    let gdir = flows.path().join("templates");
    std::fs::create_dir_all(&gdir).unwrap();
    std::fs::write(
        gdir.join("feature.yml"),
        "phases:\n  - name: global_phase\n",
    )
    .unwrap();
    // project override wins
    let pdir = repo.path().join(".flows").join("templates");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(
        pdir.join("feature.yml"),
        "phases:\n  - name: project_phase\n",
    )
    .unwrap();

    let (phases, _) = load_template(Some("feature"), repo.path(), Some(flows.path())).unwrap();
    assert_eq!(phases[0].name, "project_phase");
}

#[test]
fn list_templates_includes_builtins() {
    let flows = tempdir().unwrap();
    let names = list_templates(Some(flows.path()), None);
    for n in ["blank", "feature", "research", "review", "software"] {
        assert!(names.contains(&n.to_string()), "missing {n}");
    }
}

#[test]
fn list_templates_project_first() {
    let repo = tempdir().unwrap();
    let flows = tempdir().unwrap();
    let pdir = repo.path().join(".flows").join("templates");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(pdir.join("custom.yml"), "phases: []\n").unwrap();
    let names = list_templates(Some(flows.path()), Some(repo.path()));
    assert_eq!(names[0], "custom"); // project entries come first
    assert!(names.contains(&"feature".to_string()));
}
