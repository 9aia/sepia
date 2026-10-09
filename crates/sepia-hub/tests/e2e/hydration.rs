//! Hydration gate — every route must hydrate without a single SEVERE
//! console entry (tachys panics log as `internal error: entered
//! unreachable code` / `Unrecoverable hydration error`).

use std::time::Instant;

use crate::harness::{self, Browser, E2eEnv, WAIT};

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn every_route_hydrates_without_console_errors() {
    if !harness::browser_enabled() {
        eprintln!("skipping: SEPIA_BROWSER_E2E=1 + chromedriver + chrome required");
        return;
    }
    let env = E2eEnv::spawn().await;
    let browser = Browser::connect((1280, 900)).await;
    let deadline = Instant::now() + WAIT;

    for path in harness::ROUTES {
        let url = format!("{}{}", env.hub_url.trim_end_matches('/'), path);
        browser.driver.goto(&url).await.unwrap();
        // Hydration runs right after the bundle loads; give it a beat,
        // then drain the console.
        let _ = harness::wait_elem(&browser.driver, "body", deadline).await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let severe = browser.severe_logs().await;
        assert!(
            severe.is_empty(),
            "{path}: SEVERE console entries:\n{}",
            severe.join("\n")
        );
    }

    browser.shutdown().await;
    let (hub_err, _) = env.shutdown();
    assert!(
        !hub_err.contains("panicked"),
        "hub worker panicked during the sweep:\n{hub_err}"
    );
}
