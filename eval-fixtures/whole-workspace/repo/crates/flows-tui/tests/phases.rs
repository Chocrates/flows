#[test]
fn loop_edge_annotates_and_spans() {
    use flows_tui::phases::LoopEdge;
    let mut p = flows_tui::PhasePanel::new();
    p.setup(&["implement".into(), "test".into(), "review".into()]);
    p.set_loops(vec![LoopEdge {
        from: "review".into(),
        to: "implement".into(),
        max: 3,
    }]);

    assert!(p.has_loops());
    // label persistently shows the iteration budget
    let rows = p.rows();
    assert!(rows.iter().any(|(_, l, _)| l == "review ↺0/3"));
    // span: target(row0) → source(row2), not yet active
    assert_eq!(p.loop_spans(), vec![(0, 2, false)]);

    // once looping, the count updates + span goes active
    p.set_loop_iteration("review", 2);
    let rows = p.rows();
    assert!(rows.iter().any(|(_, l, _)| l == "review ↺2/3"));
    assert_eq!(p.loop_spans(), vec![(0, 2, true)]);
}
