use webview_core::{EgoError, ErrorCode, Ownership};

#[test]
fn errors_serialize_as_the_published_ego_codes() {
    let error = EgoError::new(
        ErrorCode::TaskSpaceUserInControl,
        "the user holds task space 1",
    );
    assert_eq!(
        serde_json::to_string(&error).unwrap(),
        r#"{"code":"EGO_TASK_SPACE_USER_IN_CONTROL","message":"the user holds task space 1"}"#
    );
    assert_eq!(
        error.to_string(),
        "EGO_TASK_SPACE_USER_IN_CONTROL: the user holds task space 1"
    );
}

#[test]
fn ownership_round_trips() {
    let json = serde_json::to_string(&Ownership::User).unwrap();
    assert_eq!(json, r#""User""#);
    assert_eq!(
        serde_json::from_str::<Ownership>(&json).unwrap(),
        Ownership::User
    );
}
