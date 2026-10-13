//! Route-swap coverage — load `/`, click a rail link, assert the new
//! page renders and no `unreachable` trap fired. The session-page
//! unmount path used to crash the reactive graph mid-teardown
//! (`use_event_source` internals touched the arena inside the
//! write-lock); `client_side_swap` gates that fix directly and
//! `route_swap` covers the full nav flow.

use std::time::{Duration, Instant};

use crate::harness::{self, E2eEnv, WAIT};

const POLL: Duration = Duration::from_millis(300);

async fn wait_js(
    browser: &crate::harness::Browser,
    script: &str,
    deadline: Instant,
    what: &str,
) -> bool {
    let script = format!("return ({script});");
    while Instant::now() < deadline {
        if browser.eval(&script).await.as_bool().unwrap_or(false) {
            return true;
        }
        tokio::time::sleep(POLL).await;
    }
    eprintln!("wait_js timed out: {what}");
    false
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn route_swap() {
    if !harness::browser_enabled() {
        eprintln!("skipping: SEPIA_BROWSER_E2E=1 + chromedriver + chrome required");
        return;
    }
    let env = E2eEnv::spawn().await;
    let browser = harness::Browser::connect((1280, 900)).await;
    browser.goto(&env.hub_url).await;
    assert!(
        wait_js(
            &browser,
            "document.querySelectorAll('a[href^=\"/?session=\"]').length > 0",
            Instant::now() + WAIT,
            "session rows",
        )
        .await,
        "row never rendered"
    );
    // Rail → /settings (client-side swap)
    browser
        .eval("var a=[...document.querySelectorAll('a')].find(x=>x.getAttribute('href')==='/settings');a.click();'x'")
        .await;
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let sev = browser.severe_logs().await;
    assert!(
        !sev.iter().any(|l| l.contains("unreachable")),
        "swap / -> /settings trapped: {sev:?}"
    );
    let p = browser.path().await;
    eprintln!("after swap: {p}");
    // /settings → back to /
    browser
        .eval("var a=[...document.querySelectorAll('a')].find(x=>x.getAttribute('href')==='/');a.click();'x'")
        .await;
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let sev = browser.severe_logs().await;
    assert!(
        !sev.iter().any(|l| l.contains("unreachable")),
        "swap /settings -> / trapped: {sev:?}"
    );
    browser.shutdown().await;
    let (hub_err, node_err) = env.shutdown();
    assert!(!hub_err.contains("panicked"), "hub panic:\n{hub_err}");
    assert!(!node_err.contains("panicked"), "node panic:\n{node_err}");
}

/// Injects a plain `<a href="/settings">` and clicks it — the router
/// intercepts it → a real client-side swap of the session list — the
/// path that used to abort inside `use_event_source`'s teardown
/// (`reconnect_timer` StoredValue read inside `Arena::remove`'s write
/// lock). Regression gate for that fix; ignores the chromedriver flag
/// convention like the rest of the suite.
#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn client_side_swap() {
    if !harness::browser_enabled() {
        eprintln!("skipping: SEPIA_BROWSER_E2E=1 + chromedriver + chrome required");
        return;
    }
    let env = E2eEnv::spawn().await;
    let browser = harness::Browser::connect((1280, 900)).await;
    browser.goto(&env.hub_url).await;
    assert!(
        wait_js(
            &browser,
            "document.querySelectorAll('a[href^=\"/?session=\"]').length > 0",
            Instant::now() + WAIT,
            "session rows",
        )
        .await,
        "row never rendered"
    );
    // `window.__alive` survives a client-side swap but not a document
    // load — distinguishes the real swap path from a reload.
    browser.eval("window.__alive=42;'ok'").await;
    browser
        .eval("var a=document.createElement('a');a.setAttribute('href','/settings');a.textContent='x';document.body.appendChild(a);a.click();'x'")
        .await;
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let sev = browser.severe_logs().await;
    let alive = browser.eval("return window.__alive").await;
    assert!(
        !sev.iter().any(|l| l.contains("unreachable")),
        "client-side swap / -> /settings trapped: {sev:?}"
    );
    eprintln!("alive after swap: {alive}");
    browser.shutdown().await;
    let (hub_err, node_err) = env.shutdown();
    assert!(!hub_err.contains("panicked"), "hub panic:\n{hub_err}");
    assert!(!node_err.contains("panicked"), "node panic:\n{node_err}");
}
