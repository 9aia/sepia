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
