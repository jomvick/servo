use webview_core::{ErrorCode, ProfileId, Registry, SpaceId, TabId};

fn registry_with_space() -> (Registry, SpaceId) {
    let mut registry = Registry::default();
    let space = registry.create_space("checkout", ProfileId("default".into()));
    (registry, space)
}

#[test]
fn hand_off_blocks_agent_mutations() {
    let (mut registry, space) = registry_with_space();
    registry.claim(space).unwrap();
    registry.hand_off(space).unwrap();
    assert_eq!(
        registry.create_tab(space, None).unwrap_err().code,
        ErrorCode::TaskSpaceUserInControl
    );
    assert!(registry.space(space).unwrap().tabs.is_empty());
}

#[test]
fn take_over_and_complete_restore_control() {
    let (mut registry, space) = registry_with_space();
    registry.claim(space).unwrap();
    registry.hand_off(space).unwrap();
    registry.take_over(space).unwrap();
    assert!(registry.create_tab(space, None).is_ok());
    registry.complete(space).unwrap();
    assert_eq!(
        registry.create_tab(space, None).unwrap_err().code,
        ErrorCode::TaskSpaceInactive
    );
}

#[test]
fn close_removes_tabs_and_unknown_ids_are_reported() {
    let (mut registry, space) = registry_with_space();
    let tab = registry.create_tab(space, None).unwrap();
    assert_eq!(registry.close(space).unwrap(), vec![tab]);
    assert_eq!(
        registry.tab(tab).unwrap_err().code,
        ErrorCode::WebContentsUnavailable
    );
    assert_eq!(
        registry.space(SpaceId(99)).unwrap_err().code,
        ErrorCode::TaskSpaceNotFound
    );
}

#[test]
fn closing_the_active_tab_promotes_the_last_remaining_one() {
    let (mut registry, space) = registry_with_space();
    let first = registry.create_tab(space, None).unwrap();
    let second = registry.create_tab(space, None).unwrap();
    assert_eq!(registry.space(space).unwrap().active, Some(first));

    registry.close_tab(first).unwrap();
    assert_eq!(registry.space(space).unwrap().active, Some(second));
    assert_eq!(registry.space(space).unwrap().tabs, vec![second]);

    registry.close_tab(second).unwrap();
    assert_eq!(registry.space(space).unwrap().active, None);

    // Un onglet parti n'est plus atteignable, même depuis un space tenu par l'utilisateur.
    registry.hand_off(space).unwrap();
    assert_eq!(
        registry.tab(first).unwrap_err().code,
        ErrorCode::WebContentsUnavailable
    );
    assert!(registry.close_tab(first).is_err());
}

#[test]
fn a_completed_space_can_still_be_closed() {
    let (mut registry, space) = registry_with_space();
    let tab = registry.create_tab(space, None).unwrap();
    registry.complete(space).unwrap();
    assert_eq!(registry.close(space).unwrap(), vec![tab]);
    assert_eq!(
        registry.space(space).unwrap_err().code,
        ErrorCode::TaskSpaceNotFound
    );
}

#[test]
fn agent_control_is_required_before_touching_a_users_space() {
    let (mut registry, space) = registry_with_space();
    registry.hand_off(space).unwrap();
    assert_eq!(
        registry.require_agent_control(space).unwrap_err().code,
        ErrorCode::TaskSpaceUserInControl
    );
    registry.claim(space).unwrap();
    assert_eq!(registry.require_agent_control(space).unwrap().id, space);
}

#[test]
fn the_engine_writes_what_it_observes_on_a_tab() {
    let (mut registry, space) = registry_with_space();
    let tab = registry.create_tab(space, None).unwrap();

    registry.tab_mut(tab).unwrap().url = Some("https://example.test".to_owned());
    registry.tab_mut(tab).unwrap().title = Some("Example".to_owned());
    registry.tab_mut(tab).unwrap().loading = false;
    assert_eq!(registry.tab(tab).unwrap().title.as_deref(), Some("Example"));

    registry.set_active_tab(tab).unwrap();
    assert_eq!(registry.space(space).unwrap().active, Some(tab));
    assert!(registry.tab(TabId(999)).is_err());
    assert_eq!(
        registry.tab_mut(TabId(999)).unwrap_err().code,
        ErrorCode::WebContentsUnavailable
    );
}

#[test]
fn user_can_open_tabs_after_handoff_but_not_after_completion() {
    let (mut registry, space) = registry_with_space();
    registry.hand_off(space).unwrap();
    let tab = registry
        .create_user_tab(space, Some("about:blank".into()))
        .unwrap();
    assert_eq!(registry.tab(tab).unwrap().space, space);
    assert_eq!(
        registry.create_tab(space, None).unwrap_err().code,
        ErrorCode::TaskSpaceUserInControl
    );
    registry.take_over(space).unwrap();
    registry.complete(space).unwrap();
    assert_eq!(
        registry.create_user_tab(space, None).unwrap_err().code,
        ErrorCode::TaskSpaceInactive
    );
    assert_eq!(
        registry
            .create_user_tab(SpaceId(u64::MAX), None)
            .unwrap_err()
            .code,
        ErrorCode::TaskSpaceNotFound
    );
}
