use std::time::{Duration, Instant};

use serde_json::{Value, json};
use webview_runtime::{Browser, RuntimeConfig};

fn call(client: &webview_runtime::BrowserClient, method: &str, params: Value) -> Value {
    client
        .request(method, params)
        .unwrap_or_else(|error| panic!("{method}: {error}"))
}

#[test]
fn standalone_engine_contract() {
    let profile = std::env::temp_dir().join(format!("ego-runtime-test-{}", std::process::id()));
    let browser = Browser::start(RuntimeConfig {
        data_dir: profile.clone(),
        initial_url: "data:text/html,<title>Runtime fixture</title><body style='background:rgb(30,80,120)'><label>Name<input id='name'></label><button onclick=\"document.title='Clicked'\">Go</button>".into(),
    }).expect("start engine");
    let client = browser.client();
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let title = client.request(
            "ego.evaluate",
            json!({"tab":1,"expression":"document.title"}),
        );
        if title.as_ref().is_ok_and(|v| v == "Runtime fixture") && client.latest_frame(1).is_some()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fixture failed to load: {title:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let snapshot = call(&client, "ego.snapshot", json!({"tab":1}));
    let refs = snapshot["refs"].as_array().expect("snapshot refs");
    let button = refs
        .iter()
        .find(|r| r["role"] == "button")
        .expect("button ref")["reference"]
        .clone();
    let input = refs
        .iter()
        .find(|r| r["role"] == "textbox")
        .expect("input ref")["reference"]
        .clone();
    call(
        &client,
        "ego.fill",
        json!({"tab":1,"ref":input,"value":"Komet"}),
    );
    assert_eq!(
        call(
            &client,
            "ego.evaluate",
            json!({"tab":1,"expression":"document.getElementById('name').value"})
        ),
        "Komet"
    );
    call(&client, "ego.click", json!({"tab":1,"ref":button}));
    assert_eq!(
        call(
            &client,
            "ego.evaluate",
            json!({"tab":1,"expression":"document.title"})
        ),
        "Clicked"
    );
    call(
        &client,
        "ego.evaluate",
        json!({"tab":1,"expression":r#"(() => {
        const same = document.createElement('iframe');
        same.srcdoc = `<button onclick="document.title='Child clicked'">Child action</button>`;
        const cross = document.createElement('iframe');
        cross.src = 'data:text/html,<button>Cross origin</button>';
        for (const frame of [same, cross]) {
            frame.onload = () => frame.dataset.loaded = 'yes';
            document.body.append(frame);
        }
        return true;
    })()"#}),
    );
    while call(
        &client,
        "ego.evaluate",
        json!({"tab":1,"expression":"Array.from(document.querySelectorAll('iframe')).every(f => f.dataset.loaded === 'yes')"}),
    ) != true
    {
        assert!(Instant::now() < deadline, "iframes failed to load");
        std::thread::sleep(Duration::from_millis(20));
    }
    let frames = call(&client, "ego.snapshot", json!({"tab":1}));
    assert!(frames["text"].as_str().unwrap().contains("[unreadable]"));
    let child = frames["refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["frame"] == "0.0" && r["role"] == "button")
        .expect("same-origin child reference")["reference"]
        .clone();
    call(&client, "ego.click", json!({"tab":1,"ref":child}));
    assert_eq!(
        call(
            &client,
            "ego.evaluate",
            json!({"tab":1,"expression":"document.querySelector('iframe').contentDocument.title"})
        ),
        "Child clicked"
    );
    let capture = call(&client, "ego.capture", json!({"tab":1}));
    assert_eq!(capture["mimeType"], "image/png");
    assert!(capture["data"].as_str().unwrap().starts_with("iVBOR"));
    call(&client, "ego.handOffTaskSpace", json!({"space":1}));
    let error = client
        .request_rpc("ego.navigate", json!({"tab":1,"url":"about:blank"}))
        .unwrap_err();
    assert_eq!(error.message, "EGO_TASK_SPACE_USER_IN_CONTROL");
    call(&client, "ui.reload", json!({"tab":1}));
    let user_tab = call(
        &client,
        "ui.createTab",
        json!({"space":1,"url":"about:blank"}),
    );
    call(&client, "ui.closeTab", json!({"tab":user_tab["id"]}));
    call(&client, "ego.takeOverTaskSpace", json!({"space":1}));
    let error = client
        .request_rpc("ego.sendCDPMessage", json!({"tab":1,"message":{}}))
        .unwrap_err();
    assert_eq!(error.message, "EGO_CDP_CHANNEL_UNAVAILABLE");
    call(&client, "ego.closeTab", json!({"tab":1}));
    let error = client
        .request_rpc("ego.evaluate", json!({"tab":1,"expression":"1"}))
        .unwrap_err();
    assert_eq!(error.message, "EGO_WEB_CONTENTS_UNAVAILABLE");
    browser.shutdown().expect("shutdown");
    assert!(client.request("ego.getBrowserVersion", json!({})).is_err());
    std::fs::remove_dir_all(profile).unwrap();
}
