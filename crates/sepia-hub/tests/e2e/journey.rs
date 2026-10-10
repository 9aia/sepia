//! Critical-path journey — ONE env + ONE browser walking the whole
//! interactive surface in sequence. The per-feature tests each spawn
//! their own node+hub+Chrome (the suite's dominant cost, ~10min
//! serialized); this shares a single pair and `cargo xtask e2e` runs
//! it first for fast signal. Assertion messages name the step so a
//! failure pinpoints where the path broke.

use std::time::{Duration, Instant};

use thirtyfour::prelude::*;

use crate::harness::{self, Browser, E2eEnv, SESSION_A, TITLE_A, TITLE_B, WAIT, wait_elem};

/// Rows in the master-detail list are `<a href="/?session=…">`.
const ROW: &str = r#"a[href^="/?session="]"#;
/// The detail panel's prompt textarea.
const PROMPT: &str = r#"textarea[placeholder*="Message the agent"]"#;
/// The session-list filter box.
const FILTER: &str = r#"input[placeholder*="Filter sessions"]"#;
/// The shortcut cheat-sheet's open marker — the markup is always
/// mounted; visibility is `data-open`/opacity, so assert the marker,
/// never presence/offsetParent.
const SHEET_OPEN: &str =
    r#"return !!document.querySelector('[data-name="ShortcutsHelp"][data-open]')"#;

/// JS click — the webdriver hit-test path scrolls and stalls the
/// renderer under load; `el.click()` fires the same handlers.
async fn js_click(driver: &WebDriver, el: &WebElement) {
    let _ = driver
        .execute("arguments[0].click()", vec![el.to_json().unwrap()])
        .await;
}

/// Any session row containing `title` currently in the DOM?
async fn row_has(driver: &WebDriver, title: &str) -> bool {
    for row in driver.find_all(By::Css(ROW)).await.unwrap_or_default() {
        if row.text().await.unwrap_or_default().contains(title) {
            return true;
        }
    }
    false
}

/// Poll until the hydrate bundle has run — `hydrate()` registers the
/// service worker after mounting `App`, so a resolved registration
/// means every document listener is attached. Under load wasm
/// instantiation alone can eat most of a 30s wait.
async fn hydrated(browser: &Browser, deadline: Instant) -> bool {
    while Instant::now() < deadline {
        let regs = browser
            .eval_async(
                "const done = arguments[arguments.length - 1];                  navigator.serviceWorker.getRegistrations()                     .then(r => done(r.length)).catch(() => done(0))",
            )
            .await;
        if regs.as_i64().unwrap_or(0) >= 1 {
            return true;
        }
        tokio::time::sleep(harness::POLL).await;
    }
    false
}

/// Dump a diagnostic blob for a stuck-step panic — active element,
/// sheet state, and the console log tail.
async fn diagnostics(browser: &Browser) -> String {
    let active = browser
        .eval("return document.activeElement && document.activeElement.tagName")
        .await;
    let open = browser.eval(SHEET_OPEN).await;
    let logs = browser
        .driver
        .browser_log()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|l| format!("[{}] {}", l.level, l.message))
        .collect::<Vec<_>>()
        .join("\n");
    format!("activeElement={active} sheetOpen={open}\nconsole:\n{logs}")
}

/// Set the filter's DOM value + dispatch `input` (the listener only
/// exists post-hydration, and `prop:value` reverts raw edits — retry).
async fn set_filter(driver: &WebDriver, value: &str) {
    driver
        .execute(
            &format!(
                r#"const el = document.querySelector('{FILTER}');
                   if (el) {{
                       el.value = arguments[0];
                       el.dispatchEvent(new Event('input', {{bubbles: true}}));
                   }}"#
            ),
            vec![serde_json::Value::String(value.into())],
        )
        .await
        .unwrap();
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn critical_path_journey() {
    if !harness::browser_enabled() {
        eprintln!("skipping: SEPIA_BROWSER_E2E=1 + chromedriver + chrome required");
        return;
    }
    let env = E2eEnv::spawn().await;
    let browser = Browser::connect((1280, 900)).await;

    // -- 1. `/` SSRs the seeded rows with a clean console. -----------
    browser.goto_ready(&env.hub_url).await;
    wait_elem(&browser.driver, ROW, Instant::now() + WAIT)
        .await
        .expect("step 1: no session rows rendered");
    // Hydration runs right after the bundle loads — give it a beat
    // before draining the console.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let severe = browser.severe_logs().await;
    assert!(
        severe.is_empty(),
        "step 1: SEVERE console entries on /:\n{}",
        severe.join("\n")
    );

    // -- 2. Row click opens the detail panel. -------------------------
    let deadline = Instant::now() + WAIT;
    let mut clicked = false;
    while Instant::now() < deadline && !clicked {
        for row in browser
            .driver
            .find_all(By::Css(ROW))
            .await
            .unwrap_or_default()
        {
            if row.text().await.unwrap_or_default().contains(TITLE_A) {
                js_click(&browser.driver, &row).await;
                clicked = true;
                break;
            }
        }
        if !clicked {
            tokio::time::sleep(harness::POLL).await;
        }
    }
    assert!(clicked, "step 2: no row contained {TITLE_A}");
    let wanted = format!("?session={SESSION_A}");
    let mut navigated = false;
    while Instant::now() < deadline {
        if browser.path().await.contains(&wanted) {
            navigated = true;
            break;
        }
        tokio::time::sleep(harness::POLL).await;
    }
    assert!(
        navigated,
        "step 2: never navigated to {wanted}: {}",
        browser.path().await
    );
    assert!(
        wait_elem(&browser.driver, PROMPT, Instant::now() + WAIT)
            .await
            .is_some(),
        "step 2: detail panel never rendered the prompt box"
    );

    // -- 3. Filter narrows the list; clearing restores both rows. ----
    // If the click was a full navigation, hydration re-attaches —
    // retry the edit until the rows actually narrow.
    let deadline = Instant::now() + WAIT;
    let filtered = loop {
        set_filter(&browser.driver, "alpha").await;
        tokio::time::sleep(harness::POLL).await;
        if !row_has(&browser.driver, TITLE_B).await {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
    };
    assert!(
        filtered,
        "step 3: filtering to 'alpha' left {TITLE_B} visible"
    );
    assert!(
        row_has(&browser.driver, TITLE_A).await,
        "step 3: {TITLE_A} missing while filtering to 'alpha'"
    );
    set_filter(&browser.driver, "").await;
    let deadline = Instant::now() + WAIT;
    let restored = loop {
        if row_has(&browser.driver, TITLE_A).await && row_has(&browser.driver, TITLE_B).await {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(harness::POLL).await;
    };
    assert!(
        restored,
        "step 3: clearing the filter did not restore both rows"
    );

    // -- 4. `?` opens the shortcut sheet; Escape closes it. -----------
    // The listener attaches at hydration — block on it explicitly
    // first (wasm instantiation under load is most of the wait).
    assert!(
        hydrated(&browser, Instant::now() + WAIT).await,
        "step 4: hydrate bundle never ran (no service worker)\n{}",
        diagnostics(&browser).await
    );
    // NOTE: `send_keys("?")` on <body> delivers keydown with `key=""`
    // under chromedriver (text-input path on a non-editable target) —
    // the app never sees "?". Dispatch a real keydown instead; it hits
    // the same document listener. `press(Escape)` is fine — named keys
    // get proper `key` values.
    let deadline = Instant::now() + WAIT;
    let mut opened = false;
    while Instant::now() < deadline {
        browser
            .eval(
                r#"document.dispatchEvent(new KeyboardEvent('keydown', {key: '?', bubbles: true}))"#,
            )
            .await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        if browser.eval(SHEET_OPEN).await.as_bool() == Some(true) {
            opened = true;
            break;
        }
    }
    assert!(
        opened,
        "step 4: '?' never opened the ShortcutsHelp sheet\n{}",
        diagnostics(&browser).await
    );
    browser.press(thirtyfour::Key::Escape).await;
    let mut closed = false;
    let esc_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < esc_deadline {
        if browser.eval(SHEET_OPEN).await.as_bool() == Some(false) {
            closed = true;
            break;
        }
        tokio::time::sleep(harness::POLL).await;
    }
    assert!(
        closed,
        "step 4: Escape never closed the ShortcutsHelp sheet\n{}",
        diagnostics(&browser).await
    );

    // -- 5. Theme toggle flips <html> class + persists. ---------------
    let deadline = Instant::now() + WAIT;
    let toggle = browser
        .visible_elem(r#"button[title^="Theme"]"#, deadline)
        .await
        .expect("step 5: theme toggle missing");
    let initial = browser
        .eval("return document.documentElement.className")
        .await;
    // Keep clicking — a cycle that lands on an identical effective
    // class (system→dark under prefers-dark) isn't a signal yet.
    let after = loop {
        js_click(&browser.driver, &toggle).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        let cur = browser
            .eval("return document.documentElement.className")
            .await;
        if cur != initial {
            break cur;
        }
        assert!(
            Instant::now() < deadline,
            "step 5: theme toggle never changed <html> class (stayed {initial:?})"
        );
    };
    let stored = browser
        .eval("return localStorage.getItem('sepia-theme') || ''")
        .await;
    assert!(
        stored.as_str().is_some_and(|s| !s.is_empty()),
        "step 5: sepia-theme never reached localStorage (got {stored:?})"
    );
    // Reload — the inline head script applies the stored class before
    // paint/hydration; poll anyway, eager `goto` can return early.
    browser.goto_ready(&env.hub_url).await;
    let deadline = Instant::now() + WAIT;
    loop {
        let cur = browser
            .eval("return document.documentElement.className")
            .await;
        if cur == after {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "step 5: theme not persisted across reload ({after:?} → {cur:?})"
        );
        tokio::time::sleep(harness::POLL).await;
    }

    // -- 6. `/sessions/:id` deep link renders the prompt. -------------
    browser
        .goto_ready(&format!("{}sessions/{SESSION_A}", env.hub_url))
        .await;
    assert!(
        wait_elem(&browser.driver, PROMPT, Instant::now() + WAIT)
            .await
            .is_some(),
        "step 6: deep-linked panel never rendered the prompt box"
    );
    // Full navigation — hydration restarts; step 7's click needs it.
    assert!(
        hydrated(&browser, Instant::now() + WAIT).await,
        "step 6: hydrate bundle never ran after deep link"
    );

    // -- 7. Mobile width → hamburger opens the nav sheet. -------------
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
    .expect("step 7: hamburger missing at 500px width");
    assert!(
        burger.is_displayed().await.unwrap_or(false),
        "step 7: hamburger hidden at 500px width"
    );
    js_click(&browser.driver, &burger).await;
    // The nav links are always mounted — assert the open marker.
    assert!(
        wait_elem(
            &browser.driver,
            r#"div[data-open] div[data-name="SheetBody"]"#,
            deadline,
        )
        .await
        .is_some(),
        "step 7: nav drawer never opened (no open SheetBody)"
    );

    // -- 8. Service worker registered; bundle URL is content-hashed. --
    assert!(
        hydrated(&browser, Instant::now() + WAIT).await,
        "step 8: service worker never registered"
    );
    let hashed = browser
        .eval(
            r#"const re = /\/pkg\/sepia_web_[0-9a-f]+\.js/;
               return re.test(document.documentElement.outerHTML)
                   || performance.getEntriesByType('resource').some(e => re.test(e.name));"#,
        )
        .await;
    assert_eq!(
        hashed,
        serde_json::Value::Bool(true),
        "step 8: no hashed /pkg/sepia_web_* bundle in DOM or performance entries"
    );

    // -- Final. No SEVERE console entries anywhere in the journey. ----
    let severe = browser.severe_logs().await;
    assert!(
        severe.is_empty(),
        "final: SEVERE console entries across the journey:\n{}",
        severe.join("\n")
    );

    browser.shutdown().await;
    let (hub_err, node_err) = env.shutdown();
    assert!(
        !hub_err.contains("panicked"),
        "hub worker panicked during the journey:\n{hub_err}"
    );
    assert!(
        !node_err.contains("panicked"),
        "node panicked during the journey:\n{node_err}"
    );
}
