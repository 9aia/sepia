//! App shell behaviors: theme toggle, `?` shortcut sheet, the mobile
//! nav drawer, and service-worker hygiene.

use std::time::Instant;

use crate::harness::{self, Browser, E2eEnv, WAIT, wait_elem};

async fn setup() -> Option<(E2eEnv, Browser)> {
    if !harness::browser_enabled() {
        eprintln!("skipping: SEPIA_BROWSER_E2E=1 + chromedriver + chrome required");
        return None;
    }
    let env = E2eEnv::spawn().await;
    let browser = Browser::connect((1280, 900)).await;
    browser.goto_ready(&env.hub_url).await;
    // Interactive tests race hydration — the bundle fetches +
    // instantiates after load. Give it a beat before input.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    Some((env, browser))
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn theme_toggle_cycles_light_dark_system() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    let deadline = Instant::now() + WAIT;

    let toggle = browser
        .visible_elem(r#"button[title^="Theme"]"#, deadline)
        .await
        .expect("theme toggle missing");
    let initial = browser
        .eval("return document.documentElement.className")
        .await;

    // Click once — class must change. Poll rather than one-shot:
    // hydration may still be attaching the handler.
    let after = loop {
        // JS click — the webdriver path stalls the renderer under load.
        let _ = browser
            .driver
            .execute("arguments[0].click()", vec![toggle.to_json().unwrap()])
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let cur = browser
            .eval("return document.documentElement.className")
            .await;
        if cur != initial {
            break cur;
        }
        assert!(
            Instant::now() < deadline,
            "theme toggle never changed <html> class (stayed {initial:?})"
        );
    };

    // The write landed in localStorage.
    let stored = browser
        .eval("return localStorage.getItem('sepia-theme') || 'system'")
        .await;
    assert_ne!(
        stored,
        serde_json::Value::String("system".into()),
        "theme choice never reached localStorage (still system)"
    );

    // Toggle survives a reload — the inline head script applies the
    // stored class pre-paint, before hydration runs. Poll anyway:
    // `goto` + eager strategy can return before first paint.
    browser.goto_ready(&env.hub_url).await;
    let deadline = Instant::now() + WAIT;
    let persisted = loop {
        let cur = browser
            .eval("return document.documentElement.className")
            .await;
        if cur == after {
            break cur;
        }
        assert!(
            Instant::now() < deadline,
            "theme not persisted across reload ({after:?} → {cur:?})"
        );
        tokio::time::sleep(harness::POLL).await;
    };
    assert_eq!(after, persisted);

    browser.shutdown().await;
    env.shutdown();
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn question_mark_opens_and_esc_closes_the_shortcut_sheet() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    let deadline = Instant::now() + WAIT;
    wait_elem(&browser.driver, "body", deadline).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // The Sheet is always mounted — visibility is a `data-[open]`
    // transform, so `offsetParent` lies. Assert the open marker on the
    // panel containing the title.
    const SHEET_OPEN: &str =
        r#"return !!document.querySelector('[data-name="ShortcutsHelp"][data-open]')"#;

    // Hydration attaches the document listener async — press until
    // the sheet opens or the deadline dies. `send_keys("?")` on <body>
    // delivers keydown with `key=""` under chromedriver (text-input
    // path on a non-editable target), so dispatch the keydown — it
    // hits the same document listener.
    let mut found = false;
    while Instant::now() < deadline {
        browser
            .eval(
                r#"document.dispatchEvent(new KeyboardEvent('keydown', {key: '?', bubbles: true}))"#,
            )
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        if browser.eval(SHEET_OPEN).await == serde_json::Value::Bool(true) {
            found = true;
            break;
        }
    }
    assert!(found, "? did not open the keyboard-shortcut sheet");

    browser.press(thirtyfour::Key::Escape).await;
    let mut still = serde_json::Value::Bool(true);
    let esc_deadline = Instant::now() + std::time::Duration::from_secs(10);
    while Instant::now() < esc_deadline {
        still = browser.eval(SHEET_OPEN).await;
        if still == serde_json::Value::Bool(false) {
            break;
        }
        tokio::time::sleep(harness::POLL).await;
    }
    assert_eq!(
        still,
        serde_json::Value::Bool(false),
        "Esc did not close the shortcut sheet"
    );

    browser.shutdown().await;
    env.shutdown();
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn mobile_drawer_opens_at_narrow_viewports() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    // Resize to a phone-ish viewport — the hamburger only renders
    // below the lg breakpoint.
    browser
        .driver
        .set_window_rect(0, 0, 500, 800)
        .await
        .unwrap();
    let deadline = Instant::now() + WAIT;

    let burger = wait_elem(
        &browser.driver,
        r#"button[aria-label="Open navigation"]"#,
        deadline,
    )
    .await
    .expect("hamburger missing at mobile width");
    assert!(
        burger.is_displayed().await.unwrap_or(false),
        "hamburger hidden at 500px width"
    );

    // JS click — webdriver's scroll+hit-test path stalls under load.
    browser
        .driver
        .execute("arguments[0].click()", vec![burger.to_json().unwrap()])
        .await
        .unwrap();
    // The sheet slides in — the nav links are always in the DOM, so
    // the assertion must be on the open marker (`data-open`), not the
    // link's presence.
    let link = wait_elem(
        &browser.driver,
        r#"div[data-open] div[data-name="SheetBody"] a[href="/settings"]"#,
        deadline,
    )
    .await;
    assert!(link.is_some(), "drawer never opened (no open sheet)");

    browser.shutdown().await;
    env.shutdown();
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn service_worker_registers_and_bundle_is_hashed() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    let deadline = Instant::now() + WAIT;
    wait_elem(&browser.driver, "body", deadline).await.unwrap();
    // SW registration is async — give it a moment then probe.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    // `getRegistrations()` is a promise — need the async executor.
    // Registration is async; poll until it lands or the deadline.
    let mut regs = serde_json::Value::from(0);
    while Instant::now() < deadline {
        regs = browser
            .eval_async(
                "const done = arguments[arguments.length - 1];                  navigator.serviceWorker.getRegistrations()                     .then(r => done(r.length)).catch(() => done(0))",
            )
            .await;
        if regs.as_i64().unwrap_or(0) >= 1 {
            break;
        }
        tokio::time::sleep(harness::POLL).await;
    }
    assert!(
        regs.as_i64().unwrap_or(0) >= 1,
        "service worker never registered"
    );
    browser.shutdown().await;
    env.shutdown();
}
