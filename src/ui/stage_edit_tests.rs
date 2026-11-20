use super::*;

#[test]
fn a_new_screen_shows_the_first_display_and_stands_apart() {
    let mut show = Show::default();
    let d = show.outputs[0].id;
    let a = add_surface(&mut show, Preset::Flat);
    let b = add_surface(&mut show, Preset::Flat);
    let j = add_surface(&mut show, Preset::Jumbotron);
    let sa = show.stage3d.surface(a).unwrap();
    let sb = show.stage3d.surface(b).unwrap();
    let sj = show.stage3d.surface(j).unwrap();
    assert_eq!(sa.name, "Screen 1");
    assert_eq!(sb.name, "Screen 2");
    assert_eq!(sj.name, "Jumbotron 1");
    assert_eq!(sa.feed.output(), Some(d));
    // On the floor, side by side; the jumbotron hangs.
    assert!((sa.placement.position[1] - 2.25 / 2.0).abs() < 1e-4);
    assert_ne!(sa.placement.position, sb.placement.position);
    assert!(sj.placement.position[1] > 4.0);
    // Every preset makes something to draw.
    for p in Preset::ALL {
        assert!(!geometry::surface_mesh(&p.shape()).is_empty(), "{p:?}");
    }
}

#[test]
fn a_display_that_shows_the_stage_is_not_offered_to_a_screen() {
    let mut show = Show::default();
    let a = show.outputs[0].id;
    let b = show.add_display();
    assert_eq!(feed_choices(&show).len(), 2);
    show.outputs.iter_mut().find(|o| o.id == a).unwrap().source = Some(StageSource::default());
    let choices = feed_choices(&show);
    assert_eq!(choices.iter().map(|(o, _)| *o).collect::<Vec<_>>(), vec![b]);
    // A new screen shows the first display that doesn't.
    assert_eq!(first_feed(&show).output(), Some(b));
    // A projector is offered with what it shows.
    let (_wide, left, _right) = show.add_blended_pair(1920, 1080, 0.1);
    let (_, label) = feed_choices(&show)
        .into_iter()
        .find(|(o, _)| *o == left)
        .unwrap();
    assert!(label.contains("whole picture"), "{label}");
}

#[test]
fn edits_from_a_view_go_through_the_show_edit_path() {
    let mut show = Show::default();
    let mut st = UiState::default();
    apply(StageEdit::AddSurface(Preset::Curved), &mut show, &mut st);
    assert_eq!(show.stage3d.surfaces.len(), 1);
    assert_eq!(st.stage.picked, Some(show.stage3d.surfaces[0].id));
    let vp = Viewpoint {
        name: "FOH".into(),
        eye: [0.0, 2.0, 20.0],
        target: [0.0, 2.0, 0.0],
        fov_deg: 50.0,
    };
    apply(StageEdit::AddViewpoint(vp.clone()), &mut show, &mut st);
    apply(StageEdit::AddViewpoint(vp), &mut show, &mut st);
    let names: Vec<&str> = show
        .stage3d
        .viewpoints
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(names, vec!["FOH", "FOH 2"]);
    // A new venue file keeps the old one's settings.
    apply(
        StageEdit::SetVenue(PathBuf::from("/venues/a.glb")),
        &mut show,
        &mut st,
    );
    show.stage3d.venue.as_mut().unwrap().units = LengthUnit::Centimetres;
    apply(
        StageEdit::SetVenue(PathBuf::from("/venues/b.obj")),
        &mut show,
        &mut st,
    );
    let v = show.stage3d.venue.as_ref().unwrap();
    assert_eq!(v.path, PathBuf::from("/venues/b.obj"));
    assert_eq!(v.units, LengthUnit::Centimetres);
    // They are edits of the show: refused in show mode like any other.
    assert!(Action::Stage(StageEdit::AddSurface(Preset::Flat)).edits_the_show());
    let mut e = crate::engine::Engine::for_tests();
    st.show_mode = true;
    crate::ui::apply(
        Action::Stage(StageEdit::AddSurface(Preset::Flat)),
        &mut show,
        &mut e,
        &mut st,
    );
    assert_eq!(
        show.stage3d.surfaces.len(),
        1,
        "show mode keeps it as it was"
    );
}
