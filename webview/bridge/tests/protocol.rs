use serde_json::{Value, json};
use webview_bridge::protocol::{Call, ErrorObject, Id, Reply, Request, rpc};
use webview_core::{EgoError, ErrorCode, SnapshotOptions, SpaceId, TabId};

fn call(method: &str, params: Value) -> Result<Call, ErrorObject> {
    Request::parse(
        &json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string(),
    )
    .expect("a well-formed envelope")
    .call()
}

fn error_of(method: &str, params: Value) -> ErrorObject {
    call(method, params).unwrap_err()
}

#[test]
fn the_v1_surface_parses_into_typed_calls() {
    let cases = [
        ("ego.getBrowserVersion", json!({}), Call::GetBrowserVersion),
        ("ego.listProfiles", json!({}), Call::ListProfiles),
        ("ego.listTaskSpaces", json!({}), Call::ListTaskSpaces),
        (
            "ego.createTaskSpace",
            json!({ "name": "checkout" }),
            Call::CreateTaskSpace {
                name: "checkout".to_owned(),
            },
        ),
        (
            "ego.claimTaskSpace",
            json!({ "space": 3 }),
            Call::ClaimTaskSpace { space: SpaceId(3) },
        ),
        (
            "ego.handOffTaskSpace",
            json!({ "space": 3 }),
            Call::HandOffTaskSpace { space: SpaceId(3) },
        ),
        (
            "ego.takeOverTaskSpace",
            json!({ "space": 3 }),
            Call::TakeOverTaskSpace { space: SpaceId(3) },
        ),
        (
            "ego.completeTaskSpace",
            json!({ "space": 3 }),
            Call::CompleteTaskSpace { space: SpaceId(3) },
        ),
        (
            "ego.closeTaskSpace",
            json!({ "space": 3 }),
            Call::CloseTaskSpace { space: SpaceId(3) },
        ),
        (
            "ego.listTabs",
            json!({ "space": 3 }),
            Call::ListTabs { space: SpaceId(3) },
        ),
        (
            "ego.createTab",
            json!({ "space": 3, "url": "https://example.test" }),
            Call::CreateTab {
                space: SpaceId(3),
                url: Some("https://example.test".to_owned()),
            },
        ),
        (
            "ego.createTab",
            json!({ "space": 3 }),
            Call::CreateTab {
                space: SpaceId(3),
                url: None,
            },
        ),
        (
            "ego.closeTab",
            json!({ "tab": 7 }),
            Call::CloseTab { tab: TabId(7) },
        ),
        (
            "ego.navigate",
            json!({ "tab": 7, "url": "https://example.test" }),
            Call::Navigate {
                tab: TabId(7),
                url: "https://example.test".to_owned(),
            },
        ),
        (
            "ego.evaluate",
            json!({ "tab": 7, "expression": "document.title" }),
            Call::Evaluate {
                tab: TabId(7),
                expression: "document.title".to_owned(),
            },
        ),
        (
            "ego.click",
            json!({ "tab": 7, "ref": "f1:e3" }),
            Call::Click {
                tab: TabId(7),
                reference: "f1:e3".to_owned(),
            },
        ),
        (
            "ego.fill",
            json!({ "tab": 7, "reference": "f1:e1", "value": "a@b.c" }),
            Call::Fill {
                tab: TabId(7),
                reference: "f1:e1".to_owned(),
                value: "a@b.c".to_owned(),
            },
        ),
        (
            "ego.type",
            json!({ "tab": 7, "text": "bonjour" }),
            Call::TypeText {
                tab: TabId(7),
                text: "bonjour".to_owned(),
            },
        ),
        (
            "ego.scroll",
            json!({ "tab": 7, "deltaY": -200.5 }),
            Call::Scroll {
                tab: TabId(7),
                delta_x: 0.0,
                delta_y: -200.5,
            },
        ),
        (
            "ego.capture",
            json!({ "tab": 7 }),
            Call::Capture { tab: TabId(7) },
        ),
        (
            "ego.sendCDPMessage",
            json!({ "tab": 7, "message": { "id": 1, "method": "Runtime.evaluate" } }),
            Call::SendCdpMessage {
                tab: TabId(7),
                message: json!({ "id": 1, "method": "Runtime.evaluate" }),
            },
        ),
    ];

    for (method, params, expected) in cases {
        assert_eq!(
            call(method, params.clone()).unwrap(),
            expected,
            "case {method} {params}"
        );
    }
}

#[test]
fn snapshot_defaults_come_from_the_model() {
    let Call::Snapshot { options, .. } = call("ego.snapshot", json!({ "tab": 7 })).unwrap() else {
        unreachable!()
    };
    assert_eq!(options, SnapshotOptions::default());

    let Call::Snapshot { options, .. } = call(
        "ego.snapshot",
        json!({ "tab": 7, "maxChars": 900, "interactiveOnly": true }),
    )
    .unwrap() else {
        unreachable!()
    };
    assert_eq!(
        options,
        SnapshotOptions {
            max_chars: 900,
            interactive_only: true
        }
    );
}

#[test]
fn both_spellings_of_the_reference_are_accepted() {
    // `ref` est le nom qu'emploie l'API d'ego ; `reference` celui du modèle.
    let with_ref = call("ego.click", json!({ "tab": 7, "ref": "f0:e1" })).unwrap();
    let with_reference = call("ego.click", json!({ "tab": 7, "reference": "f0:e1" })).unwrap();
    assert_eq!(with_ref, with_reference);

    let error = error_of("ego.click", json!({ "tab": 7 }));
    assert_eq!(error.code, rpc::INVALID_PARAMS);
}

#[test]
fn anything_outside_ego_is_method_not_found() {
    for method in [
        "snapshot",
        "ego.useTaskSpace",
        "ego.nope",
        "browser_v0.snapshot",
    ] {
        let error = error_of(method, json!({}));
        assert_eq!(
            error.code,
            rpc::METHOD_NOT_FOUND,
            "`{method}` must be refused"
        );
        assert!(error.message.contains(method), "{}", error.message);
    }
}

#[test]
fn wrong_param_types_are_invalid_params() {
    let cases = [
        ("ego.snapshot", json!({})),
        ("ego.snapshot", json!({ "tab": "7" })),
        ("ego.snapshot", json!({ "tab": 7, "maxChars": -3 })),
        (
            "ego.snapshot",
            json!({ "tab": 7, "interactiveOnly": "yes" }),
        ),
        ("ego.createTaskSpace", json!({})),
        ("ego.fill", json!({ "tab": 7, "ref": "f0:e1" })),
        ("ego.sendCDPMessage", json!({ "tab": 7 })),
    ];
    for (method, params) in cases {
        assert_eq!(
            error_of(method, params.clone()).code,
            rpc::INVALID_PARAMS,
            "`{method}` avec {params} doit être refusé"
        );
    }
}

#[test]
fn a_malformed_envelope_is_rejected_with_the_id_when_it_can_be_read() {
    // L'enveloppe n'est pas du JSON : rien à quoi répondre.
    let reply = Request::parse("{ not json").unwrap_err();
    assert_eq!(reply_error_code(&reply), rpc::PARSE_ERROR);
    assert!(matches!(reply, Reply::Failure { id: None, .. }));

    // Du JSON, mais pas de `method` : l'identifiant est lisible, on lui répond.
    let reply = Request::parse(r#"{"jsonrpc":"2.0","id":42}"#).unwrap_err();
    assert_eq!(
        reply,
        Reply::Failure {
            jsonrpc: "2.0",
            id: Some(Id::Number(42)),
            error: ErrorObject::rpc(rpc::INVALID_REQUEST, "`method` is required")
        }
    );

    // Le champ version est obligatoire côté client.
    let reply = Request::parse(r#"{"id":42,"method":"ego.listProfiles"}"#).unwrap_err();
    assert_eq!(reply_error_code(&reply), rpc::INVALID_REQUEST);

    // Une notification n'a pas d'identifiant : elle s'exécute, on ne répond pas.
    let request = Request::parse(r#"{"jsonrpc":"2.0","method":"ego.listProfiles"}"#).unwrap();
    assert_eq!(request.id, None);
    assert_eq!(request.call().unwrap(), Call::ListProfiles);
}

#[test]
fn request_ids_follow_json_rpc_types() {
    for raw in [
        r#"{"jsonrpc":"2.0","id":true,"method":"ego.listProfiles"}"#,
        r#"{"jsonrpc":"2.0","id":[],"method":"ego.listProfiles"}"#,
        r#"{"jsonrpc":"2.0","id":{},"method":"ego.listProfiles"}"#,
    ] {
        let reply = Request::parse(raw).unwrap_err();
        assert_eq!(reply_error_code(&reply), rpc::INVALID_REQUEST, "{raw}");
        assert!(matches!(reply, Reply::Failure { id: None, .. }), "{raw}");
    }

    for id in [json!(null), json!(-1), json!(1.5), json!(u64::MAX)] {
        let raw = json!({"jsonrpc":"2.0","id":id,"method":"ego.listProfiles"}).to_string();
        let request = Request::parse(&raw).unwrap();
        assert_eq!(serde_json::to_value(request.id.unwrap()).unwrap(), id);
        let reply = webview_bridge::protocol::handle_json_rpc(&raw, |_| Ok(json!([]))).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&reply).unwrap()["id"],
            id
        );
    }

    assert_eq!(
        Request::parse(r#"{"jsonrpc":"2.0","id":"","method":"ego.listProfiles"}"#)
            .unwrap()
            .id,
        Some(Id::Text(String::new()))
    );
    assert_eq!(
        Request::parse(r#"{"jsonrpc":"2.0","id":0,"method":"ego.listProfiles"}"#)
            .unwrap()
            .id,
        Some(Id::Number(0))
    );
}

#[test]
fn scalar_params_are_invalid_requests_and_notification_failures_stay_silent() {
    for params in ["null", "true", "7", r#""text""#] {
        let raw = format!(
            r#"{{"jsonrpc":"2.0","id":"x","method":"ego.listProfiles","params":{params}}}"#
        );
        let reply = Request::parse(&raw).unwrap_err();
        assert_eq!(reply_error_code(&reply), rpc::INVALID_REQUEST, "{raw}");
    }

    let called = std::cell::Cell::new(false);
    let response = webview_bridge::protocol::handle_json_rpc(
        r#"{"jsonrpc":"2.0","method":"ego.createTaskSpace"}"#,
        |_| {
            called.set(true);
            Ok(json!(null))
        },
    );
    assert!(response.is_none());
    assert!(!called.get());
}

#[test]
fn an_ego_error_keeps_its_published_code() {
    let error = EgoError::new(
        ErrorCode::TaskSpaceUserInControl,
        "the user holds task space 1",
    );
    let reply = Reply::error(Some(Id::Number(4)), ErrorObject::ego(&error));
    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "error": {
                "code": -32000,
                "message": "EGO_TASK_SPACE_USER_IN_CONTROL",
                "data": { "code": "EGO_TASK_SPACE_USER_IN_CONTROL", "message": "the user holds task space 1" }
            }
        })
    );
}

#[test]
fn a_success_reply_is_plain_json_rpc() {
    let reply = Reply::ok(Id::Text("req-1".to_owned()), json!({ "version": "0.7.0" }));
    assert_eq!(
        reply.to_json(),
        r#"{"jsonrpc":"2.0","id":"req-1","result":{"version":"0.7.0"}}"#
    );
}

fn reply_error_code(reply: &Reply) -> i64 {
    match reply {
        Reply::Failure { error, .. } => error.code,
        Reply::Success { .. } => panic!("attendu une réponse d'erreur"),
    }
}
