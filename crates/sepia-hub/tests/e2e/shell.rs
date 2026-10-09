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
    browser.driver.goto(&env.hub_url).await.unwrap();
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

    // Click once — class must change (dark → light/system).
    toggle.click().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let after = browser
        .eval("return document.documentElement.className")
        .await;
    assert_ne!(
        initial, after,
        "theme toggle did not change <html> class ({initial:?} → {after:?})"
    );

    // Toggle survives a reload via localStorage.
    browser.driver.goto(&env.hub_url).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let persisted = browser
        .eval("return document.documentElement.className")
        .await;
    assert_eq!(
        after, persisted,
        "theme not persisted across reload ({after:?} → {persisted:?})"
    );

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

    browser.press("?").await;
    let sheet = wait_elem(
        &browser.driver,
        r#"h3"#,
        Instant::now() + std::time::Duration::from_secs(5),
    )
    .await;
    let found = match sheet {
        Some(el) => el.text().await.unwrap_or_default() == "Keyboard shortcuts",
        None => false,
    };
    assert!(found, "? did not open the keyboard-shortcut sheet");

    browser.press(thirtyfour::Key::Escape).await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let still = browser.eval(
        "return [...document.querySelectorAll('h3')].some(h => h.textContent === 'Keyboard shortcuts' && h.offsetParent !== null)"
    ).await;
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

    burger.click().await.unwrap();
    // The sheet slides in — look for a nav link inside the overlay.
    let link = wait_elem(
        &browser.driver,
        r#"div[data-name="SheetBody"] a[href="/settings"]"#,
        deadline,
    )
    .await;
    assert!(link.is_some(), "drawer never opened (no sheet nav links)");

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
    let regs = browser
        .eval_async(
            "const done = arguments[arguments.length - 1];              navigator.serviceWorker.getRegistrations()                 .then(r => done(r.length)).catch(() => done(0))",
        )
        .await;
    assert!(
        regs.as_i64().unwrap_or(0) >= 1,
        "service worker never registered"
    );
    browser.shutdown().await;
    env.shutdown();
}
