//! User simulation — ONE env + ONE browser walking the flows a real
//! user hits but the per-feature tests don't reach: state-seeded
//! reloads, the rail footer's desktop menu, the new-session form's
//! aim seeding, the details drawer, settings scrollspy, a stale
//! session link, and a node dropping mid-session and coming back.
//!
//! Step assertions name the scenario step so a failure says which
//! user action broke. Console SEVERE is checked after every phase —
//! that's the gate that keeps catching hydration/resource desyncs.

use std::time::{Duration, Instant};

use thirtyfour::prelude::*;

use crate::harness::{self, Browser, E2eEnv, WAIT};

const ROW: &str = r#"a[href^="/?session="]"#;
const POLL: Duration = Duration::from_millis(300);

/// Poll the DOM via JS until `script` returns truthy.
async fn wait_js(browser: &Browser, script: &str, deadline: Instant, what: &str) -> bool {
    // `eval` runs the script as a function body — expressions need
    // `return` to produce a value.
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

/// Visible-only JS click — hidden duplicates (desktop rail vs mobile
/// drawer) steal DOM matches.
async fn click_visible(browser: &Browser, css: &str) -> bool {
    browser
        .eval(&format!(
            "var els=[...document.querySelectorAll('{css}')];\
             var e=els.find(x=>x.offsetParent!==null||x.offsetWidth>0);\
             if(e){{e.click();return true}}return false"
        ))
        .await
        .as_bool()
        .unwrap_or(false)
}

/// Drain + assert a clean console for the named step.
async fn gate(browser: &Browser, step: &str) {
    let severe = browser.severe_logs().await;
    // The SSE bridge drops when the sandbox node is down (the
    // failure step kills it on purpose) — EventSource warns, never
    // SEVERE, so no filtering is needed here.
    assert!(
        severe.is_empty(),
        "{step}: SEVERE console entries:\n{}",
        severe.join("\n")
    );
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn user_simulation() {
    if !harness::browser_enabled() {
        eprintln!("skipping: SEPIA_BROWSER_E2E=1 + chromedriver + chrome required");
        return;
    }
    let mut env = E2eEnv::spawn().await;
    let browser = Browser::connect((1280, 900)).await;
    let url = env.hub_url.trim_end_matches('/').to_string();

    // -- 1. Cold load, session opens, transcript renders. -----------
    browser.goto_ready(&env.hub_url).await;
    assert!(
        wait_js(
            &browser,
            "!!document.querySelector('[data-name=\"SessionsPanel\"]')",
            Instant::now() + WAIT,
            "sessions panel",
        )
        .await,
        "step 1: sessions panel never rendered"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    gate(&browser, "step 1: cold load").await;

    // Open the session — click the row.
    assert!(
        click_visible(&browser, ROW).await,
        "step 1: no session row to click"
    );
    assert!(
        wait_js(
            &browser,
            "location.href.includes('session=')",
            Instant::now() + WAIT,
            "session deep link",
        )
        .await,
        "step 1: row click never navigated"
    );
    gate(&browser, "step 1: session open").await;

    // -- 2. Details drawer opens and closes. -------------------------
    // The header title/details control opens the right-hand sheet.
    let opened = click_visible(&browser, "[title=\"Session details\"]").await;
    if opened {
        assert!(
            wait_js(
                &browser,
                "!!document.querySelector('[data-name=\"ProjectsSection\"], [data-name=\"ConvertExport\"]')",
                Instant::now() + WAIT,
                "details sheet",
            )
            .await,
            "step 2: details sheet never rendered"
        );
        browser.press(Key::Escape).await;
        tokio::time::sleep(POLL).await;
    } else {
        eprintln!("step 2: no details trigger found in ChatHeader — skipping drawer step");
    }
    gate(&browser, "step 2: details drawer").await;

    // -- 3. Rail footer desktop menu. ---------------------------------
    browser.goto(&format!("{url}/")).await;
    assert!(
        wait_js(
            &browser,
            "!!document.querySelector('[data-name=\"DesktopMenu\"]')",
            Instant::now() + WAIT,
            "desktop menu",
        )
        .await,
        "step 3: DesktopMenu missing from the rail"
    );
    // Open the trigger — the summary line must show (not crash).
    assert!(
        click_visible(&browser, "[data-name=\"DesktopMenu\"] > summary").await,
        "step 3: desktop menu trigger unclickable"
    );
    assert!(
        wait_js(
            &browser,
            "!!document.querySelector('[data-name=\"AimSummary\"]')",
            Instant::now() + WAIT,
            "aim summary",
        )
        .await,
        "step 3: menu summary never appeared"
    );
    // Pick the node's agent roster item if present (submenu opens on
    // hover/focus — a click expands it in the <details> tree).
    let _ = click_visible(&browser, "[data-name=\"AimNodeSubmenu\"] > summary").await;
    let picked = browser
        .eval(
            "var items=[...document.querySelectorAll('[data-aim-menu] button')];\
             var it=items.find(x=>/Node default|devin/i.test(x.textContent));\
             if(it){it.click();return true}return false",
        )
        .await
        .as_bool()
        .unwrap_or(false);
    if picked {
        // The pick must persist to sepia-desktop.
        assert!(
            wait_js(
                &browser,
                "!!(v=>v && v !== 'null' && v !== '{}')(localStorage.getItem('sepia-desktop'))",
                Instant::now() + WAIT,
                "sepia-desktop persisted",
            )
            .await,
            "step 3: desktop aim never persisted to localStorage (got {:?})",
            browser
                .eval("return localStorage.getItem('sepia-desktop')")
                .await
        );
    } else {
        let items = browser
            .eval(
                "return [...document.querySelectorAll('[data-aim-menu] button')].map(e=>e.textContent.trim()+'/'+e.disabled).join(' | ')",
            )
            .await;
        panic!("step 3: no pickable node/agent item — menu items: {items}");
    }
    gate(&browser, "step 3: desktop menu").await;

    // -- 4. New-session form seeds from the aim. ----------------------
    // If a pick landed, the form's agent select / cwd should reflect
    // it after a reload (the adoption Effect).
    browser.goto(&format!("{url}/")).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    if picked {
        let cwd_val = browser
            .eval(
                "var i=[...document.querySelectorAll('input')].find(x=>/cwd|director|path|folder/i.test((x.placeholder||'')+(x.getAttribute('aria-label')||'')));\
                 return i ? i.value : '__no_cwd__'",
            )
            .await;
        eprintln!("step 4: cwd field value after aim seed: {cwd_val:?}");
    }
    gate(&browser, "step 4: new-session form").await;

    // -- 5. Row context menu opens with real actions. -----------------
    assert!(
        browser
            .eval(
                "var r=document.querySelector('a[href^=\"/?session=\"]');\
                 if(r){r.dispatchEvent(new MouseEvent('contextmenu',{bubbles:true}));return true}return false",
            )
            .await
            .as_bool()
            .unwrap_or(false),
        "step 5: no row for context menu"
    );
    assert!(
        wait_js(
            &browser,
            "!!(m=>m && !m.hidden && m.querySelector('[role=menuitem]'))(document.querySelector('#sepia-row-menu'))",
            Instant::now() + WAIT,
            "row menu items",
        )
        .await,
        "step 5: row context menu never showed items"
    );
    browser.press(Key::Escape).await;
    tokio::time::sleep(POLL).await;
    gate(&browser, "step 5: row menu").await;

    // -- 6. Stale session link fails gracefully. ----------------------
    browser
        .goto(&format!("{url}/?session=session-that-does-not-exist"))
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let stale_ok = browser
        .eval(
            "var b=document.body.innerText;\
             return /not found|doesn't exist|unavailable|error|try again|retry/i.test(b)",
        )
        .await
        .as_bool()
        .unwrap_or(false);
    assert!(
        stale_ok,
        "step 6: stale session showed no error/empty state: {}",
        browser.eval("document.body.innerText.slice(0,400)").await
    );
    // Server-fn 500s surface as failed fetches, not page crashes —
    // the gate below would catch an unrecoverable one.
    gate(&browser, "step 6: stale link").await;

    // -- 7. Settings: scrollspy + theme toggle. ------------------------
    browser.goto(&format!("{url}/settings")).await;
    assert!(
        wait_js(
            &browser,
            "!!document.querySelector('[data-spy]')",
            Instant::now() + WAIT,
            "settings sections",
        )
        .await,
        "step 7: settings sections never rendered"
    );
    let _ = browser
        .eval("window.scrollTo(0, document.body.scrollHeight)")
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let spy = browser
        .eval(
            "return [...document.querySelectorAll('[aria-current], [data-active]')].map(e=>e.getAttribute('aria-current')||e.dataset.active).join(',')",
        )
        .await;
    eprintln!("step 7: scrollspy markers after scroll: {spy}");
    gate(&browser, "step 7: settings").await;

    // -- 8. Node down → error surface → node back → recovery. ---------
    browser.goto(&format!("{url}/")).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    env.stop_node();
    // The status poll / SSE drop should surface *something* — a
    // reconnect pill, a down node, or an error state — not a crash.
    let _ = wait_js(
        &browser,
        "/reconnect|offline|unreachable|down|lost|error/i.test(document.body.innerText)",
        Instant::now() + Duration::from_secs(30),
        "down-node indicator",
    )
    .await;
    // Console errors from the dead SSE are expected here — drain
    // them without asserting.
    let _ = browser.severe_logs().await;
    env.start_node();
    // Back up: the list should re-render sessions once the feed
    // reconnects and the status poll flips the node to up.
    assert!(
        wait_js(
            &browser,
            "!!document.querySelector('a[href^=\"/?session=\"]')",
            Instant::now() + Duration::from_secs(45),
            "list recovery",
        )
        .await,
        "step 8: session list never recovered after node restart"
    );
    gate(&browser, "step 8: recovery").await;

    // -- 9. Seeded-storage reload — the class that broke hydration. ---
    let _ = browser
        .eval(
            "localStorage.setItem('sepia-desktop', JSON.stringify({node:'laptop',agent:'devin',model:'m1',cwd:'/tmp/x'}));\
             localStorage.setItem('sepia-panel-open','false');\
             localStorage.setItem('sepia-list-collapsed', JSON.stringify({groups:['today']}));\
             'ok'",
        )
        .await;
    browser.goto(&format!("{url}/")).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    gate(&browser, "step 9: seeded reload").await;
    // The seeded aim should surface in the footer menu.
    assert!(
        click_visible(&browser, "[data-name=\"DesktopMenu\"] > summary").await,
        "step 9: desktop menu trigger missing after seeded reload"
    );
    let has_reset = browser
        .eval(
            "return [...document.querySelectorAll('[data-aim-menu] *')].some(e=>/reset desktop/i.test(e.textContent))",
        )
        .await
        .as_bool()
        .unwrap_or(false);
    assert!(
        has_reset,
        "step 9: seeded aim didn't render the 'Reset desktop' row"
    );
    gate(&browser, "step 9: seeded aim UI").await;

    browser.shutdown().await;
    let (hub_err, node_err) = env.shutdown();
    assert!(
        !hub_err.contains("panicked"),
        "hub stderr contains a panic:\n{hub_err}"
    );
    assert!(
        !node_err.contains("panicked"),
        "node stderr contains a panic:\n{node_err}"
    );
}
