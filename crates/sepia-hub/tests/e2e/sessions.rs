//! Session flows: list rendering, row→panel navigation, `?session=`
//! deep links, and the filter bar.

use std::time::Instant;

use thirtyfour::prelude::*;

use crate::harness::{self, Browser, E2eEnv, SESSION_A, TITLE_A, TITLE_B, WAIT, wait_elem};

/// Rows in the master-detail list are `<a href="/?session=…">`.
const ROW: &str = r#"a[href^="/?session="]"#;
/// The detail panel's prompt textarea.
const PROMPT: &str = r#"textarea[placeholder*="Prompt the agent"]"#; // the composer's live placeholder

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

/// Open the header's search popover — the filter input lives inside
/// a `<details>`, so it isn't typeable until `open` lands.
async fn open_search_menu(browser: &Browser) {
    browser
        .eval(r#"document.querySelector('[data-name="SearchMenu"]').open = true"#)
        .await;
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn filter_input_narrows_the_session_list() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    let deadline = Instant::now() + WAIT;
    wait_elem(&browser.driver, ROW, deadline).await.unwrap();
    open_search_menu(&browser).await;

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

/// JS click on the first visible match — the header menus are
/// `<details>` trees, so `summary`/`button` clicks are native.
async fn click_css(browser: &Browser, css: &str) -> bool {
    browser
        .eval(&format!(
            "var e=document.querySelector('{css}');if(e){{e.click();return true}}return false"
        ))
        .await
        .as_bool()
        .unwrap_or(false)
}

/// Click a `data-name`d menu item by its visible label.
async fn click_menu_item(browser: &Browser, data_name: &str, label: &str) -> bool {
    browser
        .eval(&format!(
            "var items=[...document.querySelectorAll('[data-name=\"{data_name}\"]')];\
             var it=items.find(x=>x.textContent.trim()==='{label}');\
             if(it){{it.click();return true}}return false"
        ))
        .await
        .as_bool()
        .unwrap_or(false)
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn filter_menu_badges_and_narrows_the_list() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    let deadline = Instant::now() + WAIT;
    wait_elem(&browser.driver, ROW, deadline).await.unwrap();
    // Hydration beat — the `<details>` handlers are native, but the
    // signal writes behind item clicks attach with the bundle.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Filters → Status submenu → "Locked": every seeded row is free,
    // so the list empties and the trigger badge appears.
    assert!(
        click_css(&browser, r#"[data-name="FilterMenu"] > summary"#).await,
        "filters trigger missing"
    );
    assert!(
        click_css(&browser, r#"[data-name="StatusSubmenu"] > summary"#).await,
        "status submenu missing"
    );
    assert!(
        click_menu_item(&browser, "StatusItem", "Locked").await,
        "Locked status item missing"
    );

    // The badge dot shows while any filter is active.
    assert!(
        wait_elem(&browser.driver, r#"[data-name="FilterBadge"]"#, deadline)
            .await
            .is_some(),
        "filter badge never appeared after picking Locked"
    );
    // …and the list narrows to nothing (seeded rows are all free).
    let emptied = {
        let deadline = Instant::now() + WAIT;
        loop {
            if browser
                .driver
                .find_all(By::Css(ROW))
                .await
                .unwrap_or_default()
                .is_empty()
            {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(harness::POLL).await;
        }
    };
    assert!(emptied, "Locked filter left session rows visible");

    // Back to "Any status" — the rows return and the badge clears.
    assert!(
        click_menu_item(&browser, "StatusItem", "Any status").await,
        "Any status item missing"
    );
    assert!(
        wait_elem(&browser.driver, ROW, Instant::now() + WAIT)
            .await
            .is_some(),
        "rows did not return after clearing the status filter"
    );
    let badge_gone = {
        let deadline = Instant::now() + WAIT;
        loop {
            if browser
                .driver
                .find_all(By::Css(r#"[data-name="FilterBadge"]"#))
                .await
                .unwrap_or_default()
                .is_empty()
            {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(harness::POLL).await;
        }
    };
    assert!(badge_gone, "filter badge stayed after clearing filters");

    let severe = browser.severe_logs().await;
    assert!(
        severe.is_empty(),
        "console errors:
{}",
        severe.join("\n")
    );
    browser.shutdown().await;
    env.shutdown();
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn sort_menu_reorders_the_session_list() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    let deadline = Instant::now() + WAIT;
    wait_elem(&browser.driver, ROW, deadline).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Sort → "Title": alpha ranks before beta regardless of recency.
    assert!(
        click_css(&browser, r#"[data-name="SortMenu"] > summary"#).await,
        "sort trigger missing"
    );
    assert!(
        click_menu_item(&browser, "SortItem", "Title").await,
        "Title sort item missing"
    );
    let sorted = {
        let deadline = Instant::now() + WAIT;
        loop {
            let rows = browser
                .driver
                .find_all(By::Css(ROW))
                .await
                .unwrap_or_default();
            if rows.len() >= 2 {
                let first = rows[0].text().await.unwrap_or_default();
                if first.contains(TITLE_A) {
                    break true;
                }
            }
            if Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(harness::POLL).await;
        }
    };
    assert!(sorted, "Title sort never put {TITLE_A} first");

    // The trigger tooltip tracks the pick — `Sort: Title`.
    let title = browser
        .eval(
            r#"return document.querySelector('[data-name="SortMenu"] > summary')
                   .getAttribute('title')"#,
        )
        .await;
    assert_eq!(title.as_str(), Some("Sort: Title"));

    let severe = browser.severe_logs().await;
    assert!(
        severe.is_empty(),
        "console errors:
{}",
        severe.join("\n")
    );
    browser.shutdown().await;
    env.shutdown();
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn new_session_button_creates_or_surfaces_the_error() {
    let Some((env, browser)) = setup().await else {
        return;
    };
    let deadline = Instant::now() + WAIT;
    wait_elem(&browser.driver, ROW, deadline).await.unwrap();

    // The button mounts once the node probe reports reachable.
    let btn = wait_elem(
        &browser.driver,
        r#"button[data-name="NewSessionButton"]"#,
        deadline,
    )
    .await
    .expect("New session button never mounted");
    assert!(
        btn.is_enabled().await.unwrap_or(false),
        "New session button mounted disabled"
    );

    // The click runs the full create path — create_session → refetch
    // → `?session=` nav (or an inline error banner if spawn fails).
    btn.click().await.unwrap();
    let outcome = {
        let deadline = Instant::now() + WAIT;
        loop {
            if browser.path().await.contains("?session=") {
                break "navigated";
            }
            let banner = browser
                .driver
                .find_all(By::Css(r#"[data-name="ErrorBanner"]"#))
                .await
                .unwrap_or_default();
            if !banner.is_empty() {
                break "error";
            }
            if Instant::now() >= deadline {
                break "nothing";
            }
            tokio::time::sleep(harness::POLL).await;
        }
    };
    match outcome {
        "navigated" => {
            // `devin` IS on this box's PATH, so the sandbox spawn
            // returned an id and the page routed to the detail — but
            // the real devin CLI's store can't serve get_session/
            // session_history for it (it isn't the store adapter),
            // so the detail panel logs two 500s while it surfaces the
            // error state. Driver artifact, not the button: drain it,
            // bounce back to the list, and gate on a clean console
            // from there.
            let _ = browser.severe_logs().await;
            browser.goto(&env.hub_url).await;
            assert!(
                wait_elem(&browser.driver, ROW, Instant::now() + WAIT)
                    .await
                    .is_some(),
                "session list empty after returning from create"
            );
        }
        "error" => {
            // Spawn failed — the banner IS the feedback; nothing more
            // to verify beyond a clean console.
        }
        _ => panic!("New session click produced no feedback (no nav, no error)"),
    }

    let severe = browser.severe_logs().await;
    assert!(
        severe.is_empty(),
        "console errors:
{}",
        severe.join("\n")
    );
    browser.shutdown().await;
    env.shutdown();
}
