//! `BrowserInstance` against the fake CDP server: tab registry, navigation, URL policy,
//! screencast flow and back-pressure, input serialization, agent tools, the user-action
//! registry, timeouts and disconnects (AC-BRW-1..3 at the adapter level).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::watch;
use yulius_browser::fake::FakeCdpServer;
use yulius_browser::{
    BrowserError, BrowserEvent, BrowserInstance, CdpConfig, CdpError, InstanceConfig, KeyInput,
    MouseInput, ScreencastConfig,
};
use yulius_contracts::browser::{
    ActionRequest, ClickParams, KeyEventKind, MouseButton, MouseEventKind, NavigateParams,
    OpenParams, SCREENSHOT_MAX_BASE64_BYTES, SNAPSHOT_MAX_BYTES, ScreenshotParams, SnapshotParams,
    TabId, TabInfo, ToolParamError, TypeParams, ViewerClientMsg, ViewerErrorCode, decode_frame,
    encode_frame,
};
use yulius_domain::ids::{SandboxId, TaskId};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Setup = Result<(FakeCdpServer, Arc<BrowserInstance>, TabId), Box<dyn std::error::Error>>;

const WAIT: Duration = Duration::from_secs(5);

fn config() -> InstanceConfig {
    InstanceConfig {
        cdp: CdpConfig {
            call_timeout: WAIT,
            ..CdpConfig::default()
        },
        load_timeout: WAIT,
        ..InstanceConfig::default()
    }
}

async fn setup_with(config: InstanceConfig) -> Setup {
    let fake = FakeCdpServer::start().await?;
    let instance =
        BrowserInstance::attach(SandboxId::parse("sbx-1")?, &fake.ws_url(), config).await?;
    let tab = instance
        .tabs()
        .first()
        .map(|tab| tab.id.clone())
        .ok_or("no initial tab")?;
    Ok((fake, instance, tab))
}

async fn setup() -> Setup {
    setup_with(config()).await
}

async fn wait_tabs(
    watch: &mut watch::Receiver<Vec<TabInfo>>,
    predicate: impl Fn(&[TabInfo]) -> bool,
) -> bool {
    let waiting = async {
        loop {
            if predicate(&watch.borrow_and_update()) {
                return true;
            }
            if watch.changed().await.is_err() {
                return false;
            }
        }
    };
    tokio::time::timeout(WAIT, waiting).await.unwrap_or(false)
}

fn frame_index(jpeg: &[u8]) -> Option<u64> {
    let text = String::from_utf8_lossy(jpeg.get(2..)?);
    text.strip_prefix("frame-")?.split(';').next()?.parse().ok()
}

fn types_of(fake: &FakeCdpServer, method: &str) -> Vec<String> {
    fake.calls_for(method)
        .iter()
        .map(|call| call.params["type"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn tabs_open_activate_close_and_follow_target_events() -> TestResult {
    let (fake, instance, first) = setup().await?;
    let initial = instance.tabs();
    assert_eq!(initial.len(), 1);
    assert!(initial[0].active);
    assert_eq!(initial[0].url, "about:blank");
    assert_eq!(initial[0].action_requested, None);

    let mut tabs = instance.subscribe_tabs();
    let opened = instance.open("example.com/docs").await?;
    assert_eq!(
        fake.calls_for("Target.createTarget")[0].params["url"],
        "https://example.com/docs"
    );
    assert!(wait_tabs(&mut tabs, |t| t.len() == 2).await);
    let listed = instance.tabs();
    let tab = listed
        .iter()
        .find(|t| t.id == opened)
        .ok_or("opened tab missing")?;
    assert!(tab.active);
    assert_eq!(tab.url, "https://example.com/docs");
    assert!(listed.iter().filter(|t| t.active).count() == 1);

    instance.activate(&first).await?;
    assert_eq!(
        fake.calls_for("Target.activateTarget")[0].params["targetId"],
        first.as_str()
    );
    assert!(instance.tabs().iter().any(|t| t.id == first && t.active));

    fake.set_target_title(opened.as_str(), "Docs").await;
    assert!(
        wait_tabs(&mut tabs, |t| t
            .iter()
            .any(|t| t.id == opened && t.title == "Docs"))
        .await,
        "title change event must reach the tab list"
    );

    instance.close(&opened).await?;
    assert_eq!(instance.tabs().len(), 1);
    assert!(!fake.target_ids().contains(&opened.as_str().to_owned()));
    let unknown = TabId::parse("NOPE")?;
    assert!(matches!(
        instance.activate(&unknown).await,
        Err(BrowserError::TabNotFound(id)) if id == unknown
    ));
    assert!(matches!(
        instance.close(&opened).await,
        Err(BrowserError::TabNotFound(_))
    ));
    assert_eq!(instance.cdp_ws_url(), fake.ws_url());
    assert_eq!(
        instance.cdp_http_url(),
        Some(format!("http://127.0.0.1:{}", fake.port()).as_str())
    );
    Ok(())
}

#[tokio::test]
async fn navigation_reuses_one_session_per_tab_and_waits_for_load() -> TestResult {
    let (fake, instance, tab) = setup().await?;
    instance.navigate(&tab, "https://example.com/a").await?;
    let navigations = fake.calls_for("Page.navigate");
    assert_eq!(navigations[0].params["url"], "https://example.com/a");
    assert!(navigations[0].session_id.is_some());
    let metrics = fake.calls_for("Emulation.setDeviceMetricsOverride");
    assert_eq!(metrics[0].params["width"], 1280);
    assert_eq!(metrics[0].params["height"], 800);
    assert_eq!(fake.calls_for("Page.enable").len(), 1);

    let navigated = instance
        .tool_navigate(&NavigateParams {
            tab_id: tab.clone(),
            url: "https://example.com/b".into(),
        })
        .await?;
    assert!(navigated.loaded);
    assert_eq!(navigated.tab.url, "https://example.com/b");
    assert_eq!(fake.calls_for("Target.attachToTarget").len(), 1);

    let opened = instance
        .tool_open(&OpenParams {
            url: "https://example.org/".into(),
        })
        .await?;
    assert!(opened.loaded);
    assert!(opened.tab.active);
    assert_eq!(opened.tab.url, "https://example.org/");
    assert_eq!(
        fake.calls_for("Target.createTarget")[0].params["url"],
        "about:blank"
    );

    match instance
        .tool_navigate(&NavigateParams {
            tab_id: tab.clone(),
            url: "https://unresolvable.invalid/".into(),
        })
        .await
    {
        Err(BrowserError::NavigationFailed(text)) => {
            assert!(text.contains("ERR_NAME_NOT_RESOLVED"), "{text}");
        }
        other => return Err(format!("expected a navigation failure, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn load_wait_is_bounded() -> TestResult {
    let (fake, instance, tab) = setup_with(InstanceConfig {
        load_timeout: Duration::from_millis(200),
        ..config()
    })
    .await?;
    fake.set_emit_load_events(false);
    let navigated = instance
        .tool_navigate(&NavigateParams {
            tab_id: tab,
            url: "https://example.com/slow".into(),
        })
        .await?;
    assert!(!navigated.loaded);
    Ok(())
}

#[tokio::test]
async fn url_policy_refuses_before_any_devtools_call() -> TestResult {
    let (fake, instance, tab) = setup().await?;
    for url in [
        "file:///etc/passwd",
        "chrome://settings",
        "javascript:alert(1)",
        "data:text/html,<b>x</b>",
        "view-source:https://example.com",
    ] {
        let results = [
            instance.open(url).await.map(|_| ()),
            instance.navigate(&tab, url).await,
            instance
                .tool_open(&OpenParams { url: url.into() })
                .await
                .map(|_| ()),
            instance
                .tool_navigate(&NavigateParams {
                    tab_id: tab.clone(),
                    url: url.into(),
                })
                .await
                .map(|_| ()),
            instance
                .apply_viewer_msg(ViewerClientMsg::Open { url: url.into() })
                .await
                .map(|_| ()),
        ];
        for result in results {
            match result {
                Err(error @ BrowserError::UrlRefused(_)) => {
                    assert_eq!(error.viewer_code(), ViewerErrorCode::UrlRefused);
                }
                other => return Err(format!("{url}: expected refusal, got {other:?}").into()),
            }
        }
    }
    assert!(fake.calls_for("Target.createTarget").is_empty());
    assert!(fake.calls_for("Page.navigate").is_empty());
    Ok(())
}

#[tokio::test]
async fn screencast_frames_flow_with_acks_and_stop_without_viewers() -> TestResult {
    let (fake, instance, tab) = setup_with(InstanceConfig {
        screencast: ScreencastConfig {
            frame_buffer: 16,
            ..ScreencastConfig::default()
        },
        ..config()
    })
    .await?;
    fake.set_frames_per_screencast(5);
    let mut frames = instance.subscribe_frames(&tab).await?;
    let mut last = None;
    for expected in 0..5 {
        let frame = tokio::time::timeout(WAIT, frames.recv())
            .await?
            .ok_or("subscription closed")?;
        assert_eq!(&frame.jpeg[..2], &[0xFF, 0xD8]);
        assert_eq!(frame_index(&frame.jpeg), Some(expected));
        assert!((frame.metadata.device_width - 1280.0).abs() < f64::EPSILON);
        last = Some(frame);
    }
    assert!(
        fake.wait_for_acks(5, WAIT).await,
        "every frame is acknowledged"
    );
    let start = fake.calls_for("Page.startScreencast");
    assert_eq!(start.len(), 1);
    assert_eq!(
        start[0].params,
        json!({"format": "jpeg", "quality": 60, "maxWidth": 1280, "maxHeight": 800, "everyNthFrame": 1})
    );
    let last = last.ok_or("no frame")?;
    let wire = encode_frame(&tab, &last.jpeg);
    assert_eq!(decode_frame(&wire)?, (tab.clone(), &last.jpeg[..]));

    let mut second = instance.subscribe_frames(&tab).await?;
    let latest = second
        .try_recv()
        .ok_or("a new viewer sees the latest frame")?;
    assert_eq!(frame_index(&latest.jpeg), Some(4));
    assert_eq!(fake.calls_for("Page.startScreencast").len(), 1);
    assert_eq!(instance.frame_subscribers(), 2);

    drop(frames);
    assert_eq!(instance.frame_subscribers(), 1);
    drop(second);
    assert!(fake.wait_for_calls("Page.stopScreencast", 1, WAIT).await);
    assert_eq!(instance.frame_subscribers(), 0);
    Ok(())
}

#[tokio::test]
async fn slow_consumer_drops_oldest_frames_without_blocking_the_reader() -> TestResult {
    let (fake, instance, tab) = setup().await?;
    fake.set_frames_per_screencast(300);
    fake.set_frame_bytes(8 * 1024);
    let mut frames = instance.subscribe_frames(&tab).await?;
    // The consumer reads nothing; acknowledgements must still flow for every frame.
    assert!(fake.wait_for_acks(300, Duration::from_secs(20)).await);
    assert_eq!(fake.frames_sent(), 300);
    instance.cdp().call("Browser.getVersion", json!({})).await?;
    let mut received = Vec::new();
    while let Some(frame) = frames.try_recv() {
        received.push(frame);
    }
    assert!(
        received.len() <= ScreencastConfig::default().frame_buffer,
        "buffered frames are bounded, got {}",
        received.len()
    );
    let newest = received.last().ok_or("no frame retained")?;
    assert_eq!(frame_index(&newest.jpeg), Some(299));
    assert!(
        frames.frames_dropped() >= 290,
        "{}",
        frames.frames_dropped()
    );
    assert_eq!(instance.cdp().dropped_acks(), 0);
    Ok(())
}

#[tokio::test]
async fn input_is_dispatched_in_order_with_cdp_shapes() -> TestResult {
    let (fake, instance, tab) = setup().await?;
    let mouse = |event, button, x| MouseInput {
        event,
        x,
        y: 20.0,
        button,
        dx: 0.0,
        dy: 0.0,
        modifiers: 0,
    };
    for input in [
        mouse(MouseEventKind::Move, MouseButton::None, 10.0),
        mouse(MouseEventKind::Down, MouseButton::Left, 10.0),
        mouse(MouseEventKind::Up, MouseButton::Left, 10.0),
        mouse(MouseEventKind::Down, MouseButton::Left, 11.0),
        mouse(MouseEventKind::Up, MouseButton::Left, 11.0),
    ] {
        instance.dispatch_mouse(&tab, input).await?;
    }
    let wheel: ViewerClientMsg = serde_json::from_str(&format!(
        r#"{{"type":"mouse","tab_id":"{tab}","event":"wheel","x":5,"y":6,"dx":0,"dy":120}}"#
    ))?;
    instance.apply_viewer_msg(wheel).await?;
    let calls = fake.calls_for("Input.dispatchMouseEvent");
    assert_eq!(
        types_of(&fake, "Input.dispatchMouseEvent"),
        [
            "mouseMoved",
            "mousePressed",
            "mouseReleased",
            "mousePressed",
            "mouseReleased",
            "mouseWheel"
        ]
    );
    let field =
        |name: &str| -> Vec<Value> { calls.iter().map(|c| c.params[name].clone()).collect() };
    assert_eq!(field("clickCount"), [0, 1, 1, 2, 2, 0].map(|n| json!(n)));
    assert_eq!(field("buttons"), [0, 1, 0, 1, 0, 0].map(|n| json!(n)));
    assert_eq!(calls[1].params["button"], "left");
    assert_eq!(calls[5].params["deltaY"], 120.0);
    assert_eq!(calls[5].params["button"], "none");

    let key = |event, key: Option<&str>, code: Option<&str>, text: Option<&str>| KeyInput {
        event,
        key: key.map(str::to_owned),
        code: code.map(str::to_owned),
        text: text.map(str::to_owned),
        modifiers: 0,
    };
    instance
        .dispatch_key(
            &tab,
            key(KeyEventKind::Down, Some("a"), Some("KeyA"), Some("a")),
        )
        .await?;
    instance
        .dispatch_key(
            &tab,
            key(KeyEventKind::Up, Some("a"), Some("KeyA"), Some("a")),
        )
        .await?;
    instance
        .dispatch_key(
            &tab,
            key(KeyEventKind::Down, Some("Enter"), Some("Enter"), None),
        )
        .await?;
    instance
        .dispatch_key(&tab, key(KeyEventKind::Down, Some("Backspace"), None, None))
        .await?;
    let char_msg: ViewerClientMsg = serde_json::from_str(&format!(
        r#"{{"type":"key","tab_id":"{tab}","event":"char","text":"ż","modifiers":8}}"#
    ))?;
    instance.apply_viewer_msg(char_msg).await?;
    let keys = fake.calls_for("Input.dispatchKeyEvent");
    assert_eq!(
        types_of(&fake, "Input.dispatchKeyEvent"),
        ["keyDown", "keyUp", "keyDown", "rawKeyDown", "char"]
    );
    assert_eq!(keys[0].params["text"], "a");
    assert_eq!(keys[0].params["windowsVirtualKeyCode"], 65);
    assert!(keys[1].params.get("text").is_none());
    assert_eq!(keys[2].params["text"], "\r");
    assert_eq!(keys[2].params["windowsVirtualKeyCode"], 13);
    assert_eq!(keys[3].params["windowsVirtualKeyCode"], 8);
    assert_eq!(keys[4].params["text"], "ż");
    assert_eq!(keys[4].params["modifiers"], 8);

    let input_calls: Vec<String> = fake
        .calls()
        .into_iter()
        .filter(|c| c.method.starts_with("Input."))
        .map(|c| c.method)
        .collect();
    assert_eq!(input_calls.len(), 11);
    assert!(
        input_calls[..6]
            .iter()
            .all(|m| m == "Input.dispatchMouseEvent")
    );

    let invalid = [
        instance
            .dispatch_key(&tab, key(KeyEventKind::Char, None, None, None))
            .await,
        instance
            .dispatch_key(
                &tab,
                KeyInput {
                    modifiers: 16,
                    ..key(KeyEventKind::Down, Some("a"), None, None)
                },
            )
            .await,
        instance
            .dispatch_key(
                &tab,
                key(KeyEventKind::Down, Some(&"k".repeat(65)), None, None),
            )
            .await,
        instance
            .dispatch_mouse(
                &tab,
                mouse(MouseEventKind::Move, MouseButton::None, f64::NAN),
            )
            .await,
        instance
            .dispatch_mouse(&tab, mouse(MouseEventKind::Move, MouseButton::None, 1.0e9))
            .await,
    ];
    for result in invalid {
        assert!(
            matches!(result, Err(BrowserError::InvalidInput(_))),
            "{result:?}"
        );
    }
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| c.method.starts_with("Input."))
            .count(),
        11
    );
    Ok(())
}

#[tokio::test]
async fn click_and_type_tools_pass_selectors_as_data() -> TestResult {
    let (fake, instance, tab) = setup().await?;
    let click = |selector: Option<&str>, x: Option<f64>, y: Option<f64>| ClickParams {
        tab_id: tab.clone(),
        selector: selector.map(str::to_owned),
        x,
        y,
    };
    let clicked = instance
        .tool_click(&click(Some("a[href='/x']"), None, None))
        .await?;
    assert_eq!((clicked.x, clicked.y), (100.5, 40.25));
    let call = &fake.calls_for("Runtime.callFunctionOn")[0];
    assert_eq!(call.params["arguments"][0]["value"], "a[href='/x']");
    let source = call.params["functionDeclaration"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !source.contains("href"),
        "selector must not be spliced into code"
    );
    assert_eq!(call.params["objectId"], "document-object");
    assert_eq!(fake.calls_for("Runtime.releaseObject").len(), 1);
    let mouse = fake.calls_for("Input.dispatchMouseEvent");
    assert_eq!(
        types_of(&fake, "Input.dispatchMouseEvent"),
        ["mouseMoved", "mousePressed", "mouseReleased"]
    );
    assert_eq!(mouse[1].params["x"], 100.5);
    assert_eq!(mouse[1].params["y"], 40.25);
    assert_eq!(mouse[1].params["clickCount"], 1);

    let point = instance
        .tool_click(&click(None, Some(5.0), Some(6.0)))
        .await?;
    assert_eq!((point.x, point.y), (5.0, 6.0));
    assert!(matches!(
        instance
            .tool_click(&click(Some("#missing"), None, None))
            .await,
        Err(BrowserError::ElementNotFound)
    ));
    assert!(matches!(
        instance
            .tool_click(&click(Some("#hidden"), None, None))
            .await,
        Err(BrowserError::ElementNotVisible)
    ));
    match instance.tool_click(&click(Some("!!bad"), None, None)).await {
        Err(BrowserError::InvalidSelector(text)) => assert!(text.contains("SyntaxError"), "{text}"),
        other => return Err(format!("expected invalid selector, got {other:?}").into()),
    }
    assert!(matches!(
        instance.tool_click(&click(None, None, None)).await,
        Err(BrowserError::ToolParams(ToolParamError::ClickTarget))
    ));

    let before = fake.calls().len();
    let typed = instance
        .tool_type(&TypeParams {
            tab_id: tab.clone(),
            text: "hello\nworld".into(),
            selector: Some("#q".into()),
        })
        .await?;
    assert_eq!(typed.chars, 11);
    let sequence: Vec<(String, Value)> = fake.calls()[before..]
        .iter()
        .filter(|c| c.method.starts_with("Input.") || c.method == "Runtime.callFunctionOn")
        .map(|c| (c.method.clone(), c.params.clone()))
        .collect();
    let methods: Vec<&str> = sequence.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(
        methods,
        [
            "Runtime.callFunctionOn",
            "Input.insertText",
            "Input.dispatchKeyEvent",
            "Input.dispatchKeyEvent",
            "Input.insertText"
        ]
    );
    assert!(
        sequence[0].1["functionDeclaration"]
            .as_str()
            .unwrap_or_default()
            .contains("focus()")
    );
    assert_eq!(sequence[1].1["text"], "hello");
    assert_eq!(sequence[2].1["text"], "\r");
    assert_eq!(sequence[3].1["type"], "keyUp");
    assert_eq!(sequence[4].1["text"], "world");
    assert!(matches!(
        instance
            .tool_type(&TypeParams {
                tab_id: tab.clone(),
                text: "x".into(),
                selector: Some("#missing".into())
            })
            .await,
        Err(BrowserError::ElementNotFound)
    ));
    Ok(())
}

#[tokio::test]
async fn snapshot_and_screenshot_tools_are_bounded() -> TestResult {
    let (fake, instance, tab) = setup().await?;
    let params = SnapshotParams {
        tab_id: tab.clone(),
    };
    let snapshot = instance.tool_snapshot(&params).await?;
    assert_eq!(snapshot.tab.id, tab);
    assert!(!snapshot.truncated);
    assert_eq!(
        snapshot.outline,
        "- RootWebArea \"Fake page\"\n  - heading \"Welcome\" [level=1]\n  - link \"Docs\"\n  - button \"Sign in\"\n"
    );

    let mut nodes = vec![
        json!({"nodeId": "1", "ignored": false, "role": {"value": "RootWebArea"},
        "name": {"value": "Big"}, "childIds": (2..5000).map(|i| i.to_string()).collect::<Vec<_>>()}),
    ];
    for i in 2..5000 {
        nodes.push(json!({"nodeId": i.to_string(), "parentId": "1", "ignored": false,
            "role": {"value": "link"}, "name": {"value": format!("Link number {i} with some text")}, "childIds": []}));
    }
    fake.set_ax_nodes(Value::Array(nodes));
    let big = instance.tool_snapshot(&params).await?;
    assert!(big.truncated);
    assert!(big.outline.len() <= SNAPSHOT_MAX_BYTES);

    fake.fail(
        "Accessibility.getFullAXTree",
        -32000,
        "Accessibility is unsupported",
    );
    let fallback = instance.tool_snapshot(&params).await?;
    assert_eq!(fallback.outline, "Fake page text");

    let shot_params = ScreenshotParams {
        tab_id: tab.clone(),
    };
    let shot = instance.tool_screenshot(&shot_params).await?;
    assert_eq!(shot.mime_type, "image/png");
    assert!(shot.data_base64.starts_with("iVBORw0KGgo"));
    fake.set_screenshot_base64_len(SCREENSHOT_MAX_BASE64_BYTES + 1);
    assert!(matches!(
        instance.tool_screenshot(&shot_params).await,
        Err(BrowserError::ResultTooLarge { limit }) if limit == SCREENSHOT_MAX_BASE64_BYTES
    ));
    Ok(())
}

#[tokio::test]
async fn user_action_requests_are_registered_published_and_cleared() -> TestResult {
    let (_fake, instance, tab) = setup().await?;
    let mut events = instance.subscribe_events();
    let task = TaskId::parse("task_7")?;
    let request = instance.request_user_action(&tab, "  Log in to AWS  ", Some(&task))?;
    let expected = ActionRequest {
        reason: "Log in to AWS".into(),
        task_id: Some("task_7".into()),
    };
    assert_eq!(request, expected);
    let listed = instance.tabs();
    assert_eq!(listed[0].action_requested.as_ref(), Some(&expected));
    assert_eq!(
        instance.subscribe_tabs().borrow()[0]
            .action_requested
            .as_ref(),
        Some(&expected)
    );
    assert_eq!(
        events.recv().await?,
        BrowserEvent::ActionRequested {
            tab_id: tab.clone(),
            reason: "Log in to AWS".into(),
            task_id: Some("task_7".into()),
        }
    );

    assert!(matches!(
        instance.request_user_action(&tab, "   ", None),
        Err(BrowserError::InvalidInput(_))
    ));
    assert!(matches!(
        instance.request_user_action(&tab, &"r".repeat(1025), None),
        Err(BrowserError::InvalidInput(_))
    ));
    assert!(matches!(
        instance.request_user_action(&TabId::parse("GONE")?, "x", None),
        Err(BrowserError::TabNotFound(_))
    ));

    assert!(instance.clear_action(&tab));
    assert!(!instance.clear_action(&tab));
    assert_eq!(instance.tabs()[0].action_requested, None);
    assert_eq!(
        events.recv().await?,
        BrowserEvent::ActionCleared {
            tab_id: tab.clone()
        }
    );

    let other = instance.open("https://aws.example.com/login").await?;
    instance.request_user_action(&other, "Approve MFA", None)?;
    let _ = events.recv().await?;
    instance.close(&other).await?;
    assert_eq!(
        events.recv().await?,
        BrowserEvent::ActionCleared { tab_id: other }
    );
    Ok(())
}

#[tokio::test]
async fn viewport_applies_to_sessions_and_screencasts_and_tabs_are_bounded() -> TestResult {
    let (fake, instance, tab) = setup_with(InstanceConfig {
        max_tabs: 2,
        ..config()
    })
    .await?;
    let _frames = instance.subscribe_frames(&tab).await?;
    instance.set_viewport(800, 600).await?;
    let metrics = fake.calls_for("Emulation.setDeviceMetricsOverride");
    let last = metrics.last().ok_or("no metrics call")?;
    assert_eq!(
        (last.params["width"].clone(), last.params["height"].clone()),
        (json!(800), json!(600))
    );
    let casts = fake.calls_for("Page.startScreencast");
    assert_eq!(casts.len(), 2);
    assert_eq!(casts[1].params["maxWidth"], 800);
    assert!(matches!(
        instance.set_viewport(10, 600).await,
        Err(BrowserError::InvalidInput(_))
    ));

    let second = instance.open("https://example.com").await?;
    instance
        .navigate(&second, "https://example.com/next")
        .await?;
    let metrics = fake.calls_for("Emulation.setDeviceMetricsOverride");
    assert_eq!(
        metrics.last().map(|c| c.params["width"].clone()),
        Some(json!(800))
    );
    match instance.open("https://example.net").await {
        Err(error @ BrowserError::TooManyTabs(2)) => {
            assert_eq!(error.viewer_code(), ViewerErrorCode::Capacity);
        }
        other => return Err(format!("expected the tab bound, got {other:?}").into()),
    }
    assert_eq!(fake.calls_for("Target.createTarget").len(), 1);
    Ok(())
}

#[tokio::test]
async fn timeouts_surface_and_the_browser_stays_usable() -> TestResult {
    let (fake, instance, tab) = setup_with(InstanceConfig {
        cdp: CdpConfig {
            call_timeout: Duration::from_millis(300),
            ..CdpConfig::default()
        },
        ..config()
    })
    .await?;
    fake.mute("Page.captureScreenshot");
    match instance
        .tool_screenshot(&ScreenshotParams {
            tab_id: tab.clone(),
        })
        .await
    {
        Err(error @ BrowserError::Cdp(CdpError::Timeout { .. })) => {
            assert_eq!(error.viewer_code(), ViewerErrorCode::Upstream);
        }
        other => return Err(format!("expected a timeout, got {other:?}").into()),
    }
    assert!(instance.is_alive());
    instance.navigate(&tab, "https://example.com").await?;
    Ok(())
}

#[tokio::test]
async fn disconnect_ends_subscriptions_and_fails_operations() -> TestResult {
    let (fake, instance, tab) = setup().await?;
    let mut frames = instance.subscribe_frames(&tab).await?;
    let mut events = instance.subscribe_events();
    fake.disconnect_all();
    tokio::time::timeout(WAIT, instance.wait_disconnected()).await?;
    assert!(!instance.is_alive());
    assert_eq!(
        tokio::time::timeout(WAIT, events.recv()).await??,
        BrowserEvent::Disconnected
    );
    tokio::time::timeout(WAIT, async { while frames.recv().await.is_some() {} }).await?;
    assert!(matches!(
        instance.open("https://example.com").await,
        Err(BrowserError::Cdp(CdpError::Closed))
    ));
    Ok(())
}
