#![allow(clippy::unwrap_used, clippy::pedantic)]

//! Real-browser e2e — headless Chrome drives the full
//! `sepia-node` → `sepia-hub` SSR stack over WebDriver (thirtyfour).
//!
//! Ignored by default. Run with:
//!
//! ```bash
//! SEPIA_BROWSER_E2E=1 cargo test -p sepia-hub --test browser_e2e -- --ignored
//! ```
//!
//! Needs a chromedriver binary (`SEPIA_CHROMEDRIVER`, else
//! `target/webdriver/chromedriver`, else `chromedriver` on PATH) and a
//! Chrome/Chromium binary (`CHROME_BIN`, else PATH/common-path probe).

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use thirtyfour::prelude::*;

const SESSION_ID: &str = "browser-e2e-1";
const TITLE: &str = "Browser e2e session";
const MESSAGE: &str = "hello from browser-e2e-1";
const WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(250);

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// An ephemeral port — racy if something binds between probe and
/// spawn, but far less collision-prone than fixed test ports.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(19515)
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(bin))
            .find(|p| p.is_file())
    })
}

fn chrome_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CHROME_BIN") {
        return Some(PathBuf::from(p));
    }
    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
    ] {
        if let Some(p) = which(name) {
            return Some(p);
        }
    }
    ["/opt/google/chrome/chrome", "/usr/bin/google-chrome"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

fn chromedriver_bin() -> Result<String, String> {
    if let Some(p) = std::env::var_os("SEPIA_CHROMEDRIVER") {
        return Ok(PathBuf::from(p).display().to_string());
    }
    let local = workspace_root().join("target/webdriver/chromedriver");
    if local.is_file() {
        return Ok(local.display().to_string());
    }
    if which("chromedriver").is_some() {
        return Ok("chromedriver".into());
    }
    Err("no chromedriver: set SEPIA_CHROMEDRIVER, drop one at \
         target/webdriver/chromedriver, or put chromedriver on PATH"
        .into())
}

/// Drain a piped stderr — call only after the child has exited,
/// otherwise the read blocks until the pipe closes.
fn drain_stderr(child: &mut Child) -> String {
    let mut buf = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut buf);
    }
    buf.trim().chars().take(4000).collect()
}

fn spawn_chromedriver(bin: &str, port: u16) -> std::io::Result<Child> {
    Command::new(bin)
        .arg(format!("--port={port}"))
        .arg("--allowed-origins=*")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
}

/// Wait for the WebDriver endpoint to answer `/status`.
fn webdriver_ready(client: &ureq::Agent, url: &str, deadline: Instant) -> bool {
    loop {
        if let Ok(mut resp) = client.get(format!("{url}/status")).call() {
            if resp.body_mut().read_to_string().is_ok() {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// Poll `hub/` until SSR renders the seeded session title — the same
/// projection-lag workaround as `sync_api.rs::hub_e2e_over_a_real_node`.
fn hub_ready(client: &ureq::Agent, hub_url: &str, deadline: Instant) -> bool {
    loop {
        if let Ok(mut resp) = client.get(hub_url.to_string()).call() {
            if let Ok(text) = resp.body_mut().read_to_string() {
                if text.contains(TITLE) {
                    return true;
                }
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// Poll until `f` yields an element, or `None` at the deadline.
async fn wait_elem(driver: &WebDriver, css: &str, deadline: Instant) -> Option<WebElement> {
    loop {
        if let Ok(e) = driver.find(By::Css(css)).await {
            return Some(e);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn drive(wd_url: &str, hub_url: &str) -> Result<(), String> {
    let mut caps = DesiredCapabilities::chrome();
    for arg in [
        "--headless=new",
        "--no-sandbox",
        "--disable-gpu",
        "--disable-dev-shm-usage",
        "--window-size=1280,900",
    ] {
        caps.add_arg(arg)
            .map_err(|e| format!("chrome arg {arg}: {e}"))?;
    }
    if let Some(bin) = chrome_binary() {
        caps.set_binary(&bin.display().to_string())
            .map_err(|e| format!("chrome binary: {e}"))?;
    }

    let driver = WebDriver::new(wd_url, caps)
        .await
        .map_err(|e| format!("new session via {wd_url}: {e}"))?;
    let result = browse(&driver, hub_url).await;
    let _ = driver.quit().await;
    result
}

async fn browse(driver: &WebDriver, hub_url: &str) -> Result<(), String> {
    let deadline = Instant::now() + WAIT;

    driver
        .goto(hub_url)
        .await
        .map_err(|e| format!("goto {hub_url}: {e}"))?;

    // The session list SSRs `li.session > a.session-link` rows.
    let link = wait_elem(driver, "ul.session-list a.session-link", deadline)
        .await
        .ok_or_else(|| "no .session-list row rendered".to_string())?;
    let row_text = link.text().await.map_err(|e| format!("row text: {e}"))?;
    if !row_text.contains(TITLE) {
        return Err(format!("session row missing {TITLE:?}: {row_text:?}"));
    }

    link.click()
        .await
        .map_err(|e| format!("click session row: {e}"))?;

    // Client-side router or a plain navigation — wait for the URL.
    let wanted = format!("/sessions/{SESSION_ID}");
    loop {
        let url = driver
            .current_url()
            .await
            .map_err(|e| format!("current_url: {e}"))?;
        if url.path() == wanted {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!("never navigated to {wanted}: {url}"));
        }
        tokio::time::sleep(POLL).await;
    }

    // Detail page: title, the seeded message, and the action row.
    let title_el = wait_elem(driver, "h1.detail-title", deadline)
        .await
        .ok_or_else(|| "no h1.detail-title".to_string())?;
    let title = title_el
        .text()
        .await
        .map_err(|e| format!("title text: {e}"))?;
    if !title.contains(TITLE) {
        return Err(format!("detail title {title:?} missing {TITLE:?}"));
    }

    let msg = wait_elem(driver, "article.msg", deadline)
        .await
        .ok_or_else(|| "no article.msg".to_string())?;
    let msg_text = msg.text().await.map_err(|e| format!("msg text: {e}"))?;
    if !msg_text.contains(MESSAGE) {
        return Err(format!("message {msg_text:?} missing {MESSAGE:?}"));
    }

    let action = wait_elem(driver, "div.actions button.action", deadline)
        .await
        .ok_or_else(|| "no .actions button.action".to_string())?;
    let action_text = action
        .text()
        .await
        .map_err(|e| format!("action text: {e}"))?;
    if !action_text.contains("Attach") {
        return Err(format!("first action {action_text:?} is not Attach"));
    }
    Ok(())
}

#[ignore = "needs chromedriver + chrome; set SEPIA_BROWSER_E2E=1"]
#[tokio::test]
async fn browser_e2e_session_list_to_detail() {
    if std::env::var("SEPIA_BROWSER_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: set SEPIA_BROWSER_E2E=1 (needs chromedriver + chrome)");
        return;
    }

    // --- seed a devin store + stage the driver, like sync_api.rs ----
    let tmp = tempfile::tempdir().unwrap();
    let driver_dir = tmp.path().join("drivers");
    let node_home = tmp.path().join("node-home");
    let db_dir = tmp.path().join("devin");
    std::fs::create_dir_all(&driver_dir).unwrap();
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&node_home).unwrap();

    let driver_bin = sepia_testkit::ensure_driver_bin("sepia-driver-devin");
    std::fs::hard_link(&driver_bin, driver_dir.join("sepia-driver-devin")).unwrap();
    let db = db_dir.join("sessions.db");
    let store = sepia_driver_devin::store::DevinStore::open(&db, false).unwrap();
    let mut session = sepia_testkit::contract::session(SESSION_ID, TITLE, 1_700_000_000.0);
    session.backend_type = "windsurf".into();
    sepia_core::storage::SessionRepository::save(&store, &session)
        .await
        .unwrap();
    drop(store);

    let node_port = free_port();
    let hub_port = free_port();
    let wd_port = free_port();
    let hub_url = format!("http://127.0.0.1:{hub_port}/");
    let wd_url = format!("http://127.0.0.1:{wd_port}");

    let mut node = Command::new(sepia_testkit::ensure_driver_bin("sepia-node"))
        .env("SEPIA_DRIVER_DIR", &driver_dir)
        .env("SEPIA_DEVIN_DB", &db)
        .env("SEPIA_HOME", &node_home)
        .env("SEPIA_META", node_home.join("meta.json"))
        .env("SEPIA_NODE", node_home.join("node.json"))
        .env("SEPIA_PORT", node_port.to_string())
        .env("SEPIA_HOST", "127.0.0.1")
        .env("HOME", tmp.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut hub = match Command::new(sepia_testkit::ensure_driver_bin("sepia-hub"))
        .env("SEPIA_NODE_URL", format!("http://127.0.0.1:{node_port}"))
        .env(
            "SEPIA_NODES",
            format!("laptop=http://127.0.0.1:{node_port}"),
        )
        .env("SEPIA_HOME", tmp.path().join("hub-home"))
        .env("SEPIA_HUB_PORT", hub_port.to_string())
        .env("SEPIA_HUB_HOST", "127.0.0.1")
        .env("HOME", tmp.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = node.kill();
            let _ = node.wait();
            panic!("spawn sepia-hub: {e}");
        }
    };

    let client = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .new_agent();
    if !hub_ready(&client, &hub_url, Instant::now() + WAIT) {
        let hub_err = drain_stderr(&mut hub);
        let node_err = drain_stderr(&mut node);
        let _ = hub.kill();
        let _ = node.kill();
        let _ = hub.wait();
        let _ = node.wait();
        panic!(
            "hub never rendered {TITLE:?}\n--- hub stderr ---\n{hub_err}\n--- node stderr ---\n{node_err}"
        );
    }

    let driver_bin = match chromedriver_bin() {
        Ok(b) => b,
        Err(e) => {
            let _ = hub.kill();
            let _ = node.kill();
            let _ = hub.wait();
            let _ = node.wait();
            panic!("{e}");
        }
    };
    let mut chromedriver = match spawn_chromedriver(&driver_bin, wd_port) {
        Ok(c) => c,
        Err(e) => {
            let _ = hub.kill();
            let _ = node.kill();
            let _ = hub.wait();
            let _ = node.wait();
            panic!("spawn chromedriver {driver_bin}: {e}");
        }
    };
    if !webdriver_ready(&client, &wd_url, Instant::now() + WAIT) {
        let wd_err = drain_stderr(&mut chromedriver);
        let _ = chromedriver.kill();
        let _ = hub.kill();
        let _ = node.kill();
        let _ = chromedriver.wait();
        let _ = hub.wait();
        let _ = node.wait();
        panic!("chromedriver never answered /status\n{wd_err}");
    }

    let result = drive(&wd_url, &hub_url).await;

    // Teardown first — a failed assert must not orphan chrome or the
    // servers (clippy::zombie_processes also wants the explicit waits).
    let _ = chromedriver.kill();
    let _ = hub.kill();
    let _ = node.kill();
    let _ = chromedriver.wait();
    let _ = hub.wait();
    let _ = node.wait();
    let wd_err = drain_stderr(&mut chromedriver);
    let hub_err = drain_stderr(&mut hub);
    let node_err = drain_stderr(&mut node);

    if let Err(e) = result {
        panic!(
            "browser e2e failed: {e}\n--- hub stderr ---\n{hub_err}\n\
             --- node stderr ---\n{node_err}\n--- chromedriver stderr ---\n{wd_err}"
        );
    }
}
