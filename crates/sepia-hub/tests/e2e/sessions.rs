//! Session flows: list rendering, row→panel navigation, `?session=`
//! deep links, and the filter bar.

use std::time::Instant;

use thirtyfour::prelude::*;

use crate::harness::{self, Browser, E2eEnv, SESSION_A, TITLE_A, TITLE_B, WAIT, wait_elem};

/// Rows in the master-detail list are `<a href="/?session=…">`.
const ROW: &str = r#"a[href^="/?session="]"#;
/// The detail panel's prompt textarea.
const PROMPT: &str = r#"textarea[placeholder*="Message the agent"]"#;

async fn setup() -> Option<(E2eEnv, Browser)> {
    if !harness::browser_enabled() {
        eprintln!("skipping: SEPIA_BROWSER_E2E=1 + chromedriver + chrome required");
        return None;
    }
    let env = E2eEnv::spawn().await;
    let browser = Browser::connect((1280, 900)).await;
    browser.goto_ready(&env.hub_url).await;
    Some((env, browser))
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn session_rows_render_and_open_the_panel() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    let deadline = Instant::now() + WAIT;

    // Both seeded sessions show up as rows — find the alpha one
    // (list order isn't guaranteed).
    wait_elem(&browser.driver, ROW, deadline)
        .await
        .expect("no session rows rendered");
    let mut clicked = false;
    for _ in 0..20 {
        for row in browser
            .driver
            .find_all(By::Css(ROW))
            .await
            .unwrap_or_default()
        {
            if row.text().await.unwrap_or_default().contains(TITLE_A) {
                // JS click — the webdriver click path scrolls + hit-tests
                // and stalls the renderer under load; `el.click()` fires
                // the same anchor navigation with none of that.
                let _ = browser
                    .driver
                    .execute("arguments[0].click()", vec![row.to_json().unwrap()])
                    .await;
                clicked = true;
            }
        }
        if clicked {
            break;
        }
        tokio::time::sleep(harness::POLL).await;
    }
    assert!(clicked, "no row contained {TITLE_A}");
    let wanted = format!("?session={SESSION_A}");
    let mut ok = false;
    while Instant::now() < deadline {
        if browser.path().await.contains(&wanted) {
            ok = true;
            break;
        }
        tokio::time::sleep(harness::POLL).await;
    }
    assert!(ok, "never navigated to {wanted}: {}", browser.path().await);
    assert!(
        wait_elem(&browser.driver, PROMPT, deadline).await.is_some(),
        "detail panel did not render the prompt box"
    );

    browser.shutdown().await;
    env.shutdown();
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn deep_link_ssrs_the_detail_panel() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    // `/sessions/:id` renders the panel standalone (SSR content
    // present without hydration).
    let body = env.get(&format!("/sessions/{SESSION_A}")).unwrap();
    assert!(body.contains(TITLE_A), "deep link missing session title");

    // And it hydrates into a working prompt.
    browser
        .goto_ready(&format!("{}sessions/{SESSION_A}", env.hub_url))
        .await;
    let deadline = Instant::now() + WAIT;
    assert!(
        wait_elem(&browser.driver, PROMPT, deadline).await.is_some(),
        "deep-linked panel never hydrated the prompt box"
    );
    let severe = browser.severe_logs().await;
    assert!(severe.is_empty(), "console errors:\n{}", severe.join("\n"));

    browser.shutdown().await;
    env.shutdown();
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn filter_input_narrows_the_session_list() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    let deadline = Instant::now() + WAIT;
    wait_elem(&browser.driver, ROW, deadline).await.unwrap();

    let filter = wait_elem(
        &browser.driver,
        r#"input[placeholder*="Filter sessions"]"#,
        deadline,
    )
    .await
    .expect("filter input missing");
    filter.send_keys("alpha").await.unwrap();

    // "beta" row should disappear once the client-side filter
    // applies. Fresh deadline — wait_elem may have eaten the first.
    let beta_gone = {
        let deadline = Instant::now() + WAIT;
        let driver = &browser.driver;
        loop {
            let rows = driver.find_all(By::Css(ROW)).await.unwrap_or_default();
            let mut has_beta = false;
            for r in rows {
                if r.text().await.unwrap_or_default().contains(TITLE_B) {
                    has_beta = true;
                }
            }
            if !has_beta {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(harness::POLL).await;
        }
    };
    assert!(beta_gone, "filtering to 'alpha' left {TITLE_B} visible");

    browser.shutdown().await;
    env.shutdown();
}

/// The row menu's current `aria-activedescendant` value.
async fn activedescendant(browser: &Browser) -> serde_json::Value {
    browser
        .eval(
            "return document.querySelector('#sepia-row-menu')\
                   .getAttribute('aria-activedescendant')",
        )
        .await
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn row_menu_navigates_and_restores_focus_by_keyboard() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    let deadline = Instant::now() + WAIT;
    wait_elem(&browser.driver, ROW, deadline).await.unwrap();
    // Hydration beat — menu handlers attach with the bundle.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Focus the first row's `⋯` and open — the keyboard path (a
    // click with no pointer coords anchors on the button's corner).
    const OPEN: &str = r#"const b =
        document.querySelector('button[data-name="RowMenuButton"]');
        b.focus(); b.click();"#;
    // The menu reports its own state — object → Value::Object.
    const STATE: &str = r#"const m = document.querySelector('#sepia-row-menu');
        return {open: m && !m.classList.contains('hidden'),
                focused: document.activeElement === m,
                ad: m && m.getAttribute('aria-activedescendant')}"#;
    let key = |k: &str| {
        format!(
            "document.querySelector('#sepia-row-menu').dispatchEvent(\
                 new KeyboardEvent('keydown', {{key: '{k}', bubbles: true}}))"
        )
    };

    // Open is followed by a deferred focus: poll for the menu taking
    // DOM focus and pointing activedescendant at the first item —
    // retry the click until hydration has attached the handlers.
    let mut opened = false;
    let mut last = serde_json::Value::Null;
    while Instant::now() < deadline {
        browser.eval(OPEN).await;
        tokio::time::sleep(harness::POLL).await;
        last = browser.eval(STATE).await;
        if last["focused"].as_bool() == Some(true)
            && last["ad"].as_str() == Some("sepia-row-menu-item-0")
        {
            opened = true;
            break;
        }
    }
    assert!(
        opened,
        "menu never opened with focus on the container; last state: {last}"
    );
    let expanded = browser
        .eval(
            r#"return document.querySelector('button[data-name="RowMenuButton"]')
            .getAttribute('aria-expanded')"#,
        )
        .await;
    assert_eq!(
        expanded.as_str(),
        Some("true"),
        "open menu did not set aria-expanded on its trigger"
    );

    // ↓ moves to item 1, End to the last, 'o' typeaheads back to
    // "Open".
    browser.eval(&key("ArrowDown")).await;
    tokio::time::sleep(harness::POLL).await;
    assert_eq!(
        activedescendant(&browser).await.as_str(),
        Some("sepia-row-menu-item-1"),
        "ArrowDown did not move the active descendant"
    );
    browser.eval(&key("End")).await;
    tokio::time::sleep(harness::POLL).await;
    assert_eq!(
        activedescendant(&browser).await.as_str(),
        Some("sepia-row-menu-item-5"),
        "End did not jump to the last item"
    );
    browser.eval(&key("o")).await;
    tokio::time::sleep(harness::POLL).await;
    assert_eq!(
        activedescendant(&browser).await.as_str(),
        Some("sepia-row-menu-item-0"),
        "typeahead 'o' did not land on Open"
    );

    // Enter activates "Open" → ?session= navigation.
    browser.eval(&key("Enter")).await;
    let mut navigated = false;
    let nav_deadline = Instant::now() + WAIT;
    while Instant::now() < nav_deadline {
        if browser.path().await.contains("?session=") {
            navigated = true;
            break;
        }
        tokio::time::sleep(harness::POLL).await;
    }
    assert!(navigated, "Enter did not activate the Open item");

    // Reopen and Escape — the menu closes and focus returns to the
    // `⋯` trigger that opened it.
    browser.eval(OPEN).await;
    let mut reopened = false;
    let reopen_deadline = Instant::now() + WAIT;
    while Instant::now() < reopen_deadline {
        if browser.eval(STATE).await["focused"].as_bool() == Some(true) {
            reopened = true;
            break;
        }
        tokio::time::sleep(harness::POLL).await;
    }
    assert!(reopened, "menu never reopened");
    browser.eval(&key("Escape")).await;
    let mut closed = false;
    let esc_deadline = Instant::now() + WAIT;
    while Instant::now() < esc_deadline {
        if browser.eval(STATE).await["open"].as_bool() == Some(false) {
            closed = true;
            break;
        }
        tokio::time::sleep(harness::POLL).await;
    }
    assert!(closed, "Escape did not close the menu");
    let back = browser
        .eval(
            r#"return document.activeElement ===
                 document.querySelector('button[data-name="RowMenuButton"]')"#,
        )
        .await;
    assert_eq!(
        back.as_bool(),
        Some(true),
        "Escape did not return focus to the ⋯ trigger"
    );
    let severe = browser.severe_logs().await;
    assert!(severe.is_empty(), "console errors:\n{}", severe.join("\n"));

    browser.shutdown().await;
    env.shutdown();
}
