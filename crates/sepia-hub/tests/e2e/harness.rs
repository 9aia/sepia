//! Shared e2e harness: seeds a devin store, boots a real `sepia-node`
//! and `sepia-hub` on ephemeral ports, and (optionally) drives a
//! headless Chrome session with browser console capture enabled.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use thirtyfour::prelude::*;
use thirtyfour::{ChromiumLikeCapabilities, TypingData};

pub const SESSION_A: &str = "browser-e2e-a";
pub const TITLE_A: &str = "Browser e2e alpha";
pub const SESSION_B: &str = "browser-e2e-b";
pub const TITLE_B: &str = "Browser e2e beta";

pub const WAIT: Duration = Duration::from_secs(30);
/// Env bring-up under parallel test load (hub init is seconds; with a
/// dozen concurrent node+hub pairs 30s is not enough).
pub const SPAWN_WAIT: Duration = Duration::from_secs(120);
pub const POLL: Duration = Duration::from_millis(250);

/// Every route the app shell serves — hydration regressions (the
/// ThemeToggle stub panic, the SendWrapper stream abort) hit all of
/// them, so the sweeps walk the full set.
pub const ROUTES: &[&str] = &[
    "/",
    "/agents",
    "/projects",
    "/nodes",
    "/settings",
    "/?session=browser-e2e-a",
    "/sessions/browser-e2e-a",
];

/// `ensure_driver_bin` shells out to `cargo build`. N test processes
/// racing it livelock on cargo's build-dir lock (seen: 11-way stall
/// for 15min). Serialize with a sentinel file: the first process to
/// need binaries builds them; everyone else waits on the sentinel.
fn warm_binaries() {
    const BINS: &[&str] = &["sepia-driver-devin", "sepia-node", "sepia-hub"];
    let target = workspace_root().join("target/debug");
    // `cargo xtask e2e` builds all three up front — trust it and skip
    // cargo entirely (the call can stall behind a package lock even
    // for a no-op).
    if std::env::var_os("SEPIA_E2E_PREBUILT").is_some() {
        for n in BINS {
            assert!(target.join(n).is_file(), "{n} missing — prebuild lied");
        }
        return;
    }
    let sentinel = target.join(".e2e-build.lock");
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&sentinel)
        {
            Ok(_) => {
                for name in BINS {
                    sepia_testkit::ensure_driver_bin(name);
                }
                let _ = std::fs::remove_file(&sentinel);
                return;
            }
            Err(_) => {
                // Winner is building — once the sentinel is gone the
                // binaries are complete, no cargo needed.
                if !sentinel.exists() && BINS.iter().all(|n| target.join(n).is_file()) {
                    return;
                }
                // Retire a sentinel left by a killed test process.
                let stale = std::fs::metadata(&sentinel)
                    .and_then(|m| m.modified())
                    .map(|t| t.elapsed().unwrap_or_default() > Duration::from_secs(1200))
                    .unwrap_or(false);
                if stale {
                    let _ = std::fs::remove_file(&sentinel);
                }
                std::thread::sleep(POLL);
            }
        }
    }
}

/// After `warm_binaries` the binaries are guaranteed built — resolve
/// the path directly instead of shelling cargo again. With nextest
/// every test is its own process, so each `ensure_driver_bin` call
/// is a fresh `cargo build` serialized behind the package lock (and
/// any `cargo-watch` elsewhere) — the dominant suite cost.
fn bin_path(name: &str) -> PathBuf {
    let path = workspace_root().join("target/debug").join(name);
    assert!(path.is_file(), "{name} missing — warm_binaries ran?");
    path
}

pub fn workspace_root() -> PathBuf {
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

fn chromedriver_bin() -> Option<String> {
    if let Some(p) = std::env::var_os("SEPIA_CHROMEDRIVER") {
        return Some(PathBuf::from(p).display().to_string());
    }
    let local = workspace_root().join("target/webdriver/chromedriver");
    if local.is_file() {
        return Some(local.display().to_string());
    }
    if which("chromedriver").is_some() {
        return Some("chromedriver".into());
    }
    None
}

/// Browser tests opt in via `SEPIA_BROWSER_E2E=1` and still
/// self-skip when the driver/browser binaries are missing.
pub fn browser_enabled() -> bool {
    // The hydrated app needs a staged wasm bundle — without
    // `cargo xtask site` the SSR emits bundle URLs that 404.
    std::env::var("SEPIA_BROWSER_E2E").ok().as_deref() == Some("1")
        && chromedriver_bin().is_some()
        && chrome_binary().is_some()
        && workspace_root().join("target/site/pkg").is_dir()
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

/// Poll `hub/` until SSR renders the seeded session — the same
/// projection-lag workaround as `sync_api.rs::hub_e2e_over_a_real_node`.
fn hub_ready(client: &ureq::Agent, hub_url: &str, deadline: Instant) -> bool {
    loop {
        if let Ok(mut resp) = client.get(hub_url.to_string()).call() {
            if let Ok(text) = resp.body_mut().read_to_string() {
                if text.contains(TITLE_A) {
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
pub async fn wait_elem(driver: &WebDriver, css: &str, deadline: Instant) -> Option<WebElement> {
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

/// A seeded node + hub pair. Drop kills the children; call
/// [`E2eEnv::shutdown`] on the success path to collect stderr.
pub struct E2eEnv {
    _tmp: tempfile::TempDir,
    pub hub_url: String,
    pub http: ureq::Agent,
    node: Child,
    hub: Child,
    node_log: PathBuf,
    hub_log: PathBuf,
}

impl E2eEnv {
    /// Seed two sessions (`browser-e2e-{a,b}`) into a devin store and
    /// boot node + hub. Blocks until the hub SSRs `TITLE_A`.
    pub async fn spawn() -> Self {
        warm_binaries();
        let tmp = tempfile::tempdir().unwrap();
        let driver_dir = tmp.path().join("drivers");
        let node_home = tmp.path().join("node-home");
        let db_dir = tmp.path().join("devin");
        std::fs::create_dir_all(&driver_dir).unwrap();
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::create_dir_all(&node_home).unwrap();

        let driver_bin = bin_path("sepia-driver-devin");

        std::fs::hard_link(&driver_bin, driver_dir.join("sepia-driver-devin")).unwrap();
        let db = db_dir.join("sessions.db");
        let store = sepia_driver_devin::store::DevinStore::open(&db, false).unwrap();
        for (id, title, activity) in [
            (SESSION_A, TITLE_A, 1_700_000_000.0),
            (SESSION_B, TITLE_B, 1_700_000_100.0),
        ] {
            let mut session = sepia_testkit::contract::session(id, title, activity);
            session.backend_type = "windsurf".into();
            // Rows link `&agent=devin` — without it the live stream
            // endpoint can't resolve a driver and 400s.
            session.agent_id = Some("devin".into());
            sepia_core::storage::SessionRepository::save(&store, &session)
                .await
                .unwrap();
        }
        drop(store);

        let node_port = free_port();
        let hub_port = free_port();
        let hub_url = format!("http://127.0.0.1:{hub_port}/");
        let node_url = format!("http://127.0.0.1:{node_port}");

        // Tracing goes to stdout — a null sink loses every diagnostic.
        // Tee both streams to log files instead.
        let node_log = tmp.path().join("node.log");
        let hub_log = tmp.path().join("hub.log");
        // One file handle shared by both streams — two independent
        // `create`s would write at offset 0 concurrently and eat
        // each other's output.
        let node_out = std::fs::File::create(&node_log).unwrap();
        let hub_out = std::fs::File::create(&hub_log).unwrap();
        let node = Command::new(bin_path("sepia-node"))
            .env("SEPIA_DRIVER_DIR", &driver_dir)
            .env("SEPIA_DEVIN_DB", &db)
            .env("SEPIA_HOME", &node_home)
            .env("SEPIA_META", node_home.join("meta.json"))
            .env("SEPIA_NODE", node_home.join("node.json"))
            .env("SEPIA_PORT", node_port.to_string())
            .env("SEPIA_HOST", "127.0.0.1")
            .env("HOME", tmp.path())
            .stdout(Stdio::from(node_out.try_clone().unwrap()))
            .stderr(Stdio::from(node_out))
            .spawn()
            .unwrap();

        let hub = match Command::new(bin_path("sepia-hub"))
            .env("SEPIA_NODE_URL", &node_url)
            .env("SEPIA_NODES", format!("laptop={node_url}"))
            .env("SEPIA_HOME", tmp.path().join("hub-home"))
            .env("SEPIA_HUB_PORT", hub_port.to_string())
            .env("SEPIA_HUB_HOST", "127.0.0.1")
            // `default_site_root` probes a *relative* `target/site` —
            // under nextest the CWD is the crate dir, so the fallback
            // lands on `public/` and the bundle URLs 404.
            .env("SEPIA_SITE_ROOT", workspace_root().join("target/site"))
            .env("HOME", tmp.path())
            .stdout(Stdio::from(hub_out.try_clone().unwrap()))
            .stderr(Stdio::from(hub_out))
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let mut node = node;
                let _ = node.kill();
                let _ = node.wait();
                panic!("spawn sepia-hub: {e}");
            }
        };

        let http = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .new_agent();
        let env = Self {
            _tmp: tmp,
            hub_url,
            http,
            node,
            hub,
            node_log,
            hub_log,
        };
        if !hub_ready(&env.http, &env.hub_url, Instant::now() + SPAWN_WAIT) {
            let (hub_err, node_err) = env.shutdown();
            panic!(
                "hub never rendered {TITLE_A:?}\n--- hub ---\n{hub_err}\n--- node ---\n{node_err}"
            );
        }
        env
    }

    /// GET a hub path and return the body. Fails the test on
    /// transport errors — a truncated chunked body (the SendWrapper
    /// stream abort) surfaces here as `Err` or a missing `</html>`.
    pub fn get(&self, path: &str) -> Result<String, String> {
        let url = format!("{}{}", self.hub_url.trim_end_matches('/'), path);
        let mut resp = self
            .http
            .get(&url)
            .call()
            .map_err(|e| format!("GET {path}: {e}"))?;
        resp.body_mut()
            .read_to_string()
            .map_err(|e| format!("GET {path} body: {e}"))
    }

    /// Kill the children and return `(hub_stderr, node_stderr)`.
    /// Tests assert `!hub.contains("panicked")` — an SSR worker panic
    /// aborts the response stream without ever failing the request.
    pub fn shutdown(mut self) -> (String, String) {
        let _ = self.hub.kill();
        let _ = self.node.kill();
        let _ = self.hub.wait();
        let _ = self.node.wait();
        let read = |p: &PathBuf| {
            std::fs::read_to_string(p)
                .unwrap_or_default()
                .chars()
                .take(6000)
                .collect()
        };
        let hub_err = read(&self.hub_log);
        let node_err = read(&self.node_log);
        (hub_err, node_err)
    }
}

impl Drop for E2eEnv {
    fn drop(&mut self) {
        let _ = self.hub.kill();
        let _ = self.node.kill();
        let _ = self.hub.wait();
        let _ = self.node.wait();
    }
}

/// chromedriver + a logged Chrome session. `console` entries are
/// drained via `browser_log()`.
pub struct Browser {
    proc: Child,
    // Held so the chrome profile dir outlives the session; its path is
    // also how we kill orphaned chrome processes (see Drop).
    profile: tempfile::TempDir,
    // Cross-process serialization — nextest runs each test in its own
    // process, so an in-process lock can't stop N concurrent Chromes
    // from renderer-timeouting each other under load.
    _serialize: std::fs::File,
    pub driver: WebDriver,
}

impl Browser {
    /// `window_size` — e.g. `(1280, 900)` desktop or `(500, 800)`
    /// mobile — controls which responsive layout SSRs/hydrates.
    pub async fn connect(window_size: (u32, u32)) -> Self {
        let wd_port = free_port();
        let wd_url = format!("http://127.0.0.1:{wd_port}");
        let bin = chromedriver_bin().expect("chromedriver not found");
        let proc = Command::new(&bin)
            .arg(format!("--port={wd_port}"))
            .arg("--allowed-origins=*")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn chromedriver {bin}: {e}"));

        let probe = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(3)))
            .build()
            .new_agent();
        if !webdriver_ready(&probe, &wd_url, Instant::now() + WAIT) {
            panic!("chromedriver never answered /status");
        }

        let mut caps = DesiredCapabilities::chrome();
        for arg in [
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            "--disable-dev-shm-usage",
        ] {
            caps.add_arg(arg).unwrap();
        }
        caps.add_arg(&format!(
            "--window-size={},{}",
            window_size.0, window_size.1
        ))
        .unwrap();
        // Isolate the profile — a reused one would carry a stale
        // service worker between runs.
        let serialize = loop {
            let lock = workspace_root().join("target/.e2e-browser.lock");
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock)
            {
                Ok(f) => break f,
                Err(_) => {
                    // Another test holds the browser — or died holding
                    // it; retire after 20min.
                    let stale = std::fs::metadata(&lock)
                        .and_then(|m| m.modified())
                        .map(|t| t.elapsed().unwrap_or_default() > Duration::from_secs(1200))
                        .unwrap_or(false);
                    if stale {
                        let _ = std::fs::remove_file(&lock);
                    }
                    std::thread::sleep(POLL);
                }
            }
        };

        let profile = tempfile::tempdir().unwrap();
        caps.add_arg(&format!("--user-data-dir={}", profile.path().display()))
            .unwrap();
        caps.set_browser_log_level(thirtyfour::LoggingPrefsLogLevel::All)
            .unwrap();
        // The SSR stream stays open past `load` (out-of-order suspense
        // fragments + SSE) — "normal" page-load strategy makes `goto`
        // time out waiting for it. Eager returns at DOMContentLoaded;
        // the tests poll for elements anyway.
        caps.set_page_load_strategy(thirtyfour::PageLoadStrategy::Eager)
            .unwrap();
        if let Some(bin) = chrome_binary() {
            caps.set_binary(&bin.display().to_string()).unwrap();
        }

        let driver = WebDriver::new(&wd_url, caps)
            .await
            .unwrap_or_else(|e| panic!("new webdriver session: {e}"));
        Self {
            proc,
            profile,
            _serialize: serialize,
            driver,
        }
    }

    /// SEVERE console entries — panics, hydration errors, failed
    /// resource loads. Tests assert this is empty; that gate would
    /// have caught every regression we've shipped to this UI.
    pub async fn severe_logs(&self) -> Vec<String> {
        self.driver
            .browser_log()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.level == "SEVERE")
            .map(|e| e.message)
            .collect()
    }

    /// Drain the current URL path+query (`?session=…`).
    pub async fn path(&self) -> String {
        self.driver
            .current_url()
            .await
            .map(|u| {
                let mut p = u.path().to_string();
                if let Some(q) = u.query() {
                    p.push('?');
                    p.push_str(q);
                }
                p
            })
            .unwrap_or_default()
    }

    /// Press a key on `<body>` (global hotkeys bind at window level).
    pub async fn press(&self, key: impl Into<TypingData>) {
        let body = self.driver.find(By::Tag("body")).await.unwrap();
        body.send_keys(key).await.unwrap();
    }

    /// `goto` with retry — under load the chrome renderer occasionally
    /// stalls past the 60s webdriver nav timeout; a fresh attempt is
    /// cheaper than a failed suite.
    pub async fn goto(&self, url: &str) {
        for attempt in 0..3u8 {
            match self.driver.goto(url).await {
                Ok(()) => return,
                Err(e) if attempt < 2 => {
                    eprintln!("goto {url} attempt {attempt}: {e}; retrying");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(e) => panic!("goto {url}: {e}"),
            }
        }
    }

    /// `goto` then wait for `<body>` — eager strategy returns at
    /// DOMContentLoaded, but under load the occasional navigation
    /// lands on nothing (renderer stall). One re-goto recovers it.
    pub async fn goto_ready(&self, url: &str) {
        self.goto(url).await;
        for _ in 0..2 {
            if wait_elem(
                &self.driver,
                "body",
                Instant::now() + Duration::from_secs(10),
            )
            .await
            .is_some()
            {
                return;
            }
            self.goto(url).await;
        }
        panic!("{url} never produced a <body>");
    }

    /// Evaluate JS and return the JSON value.
    pub async fn eval(&self, script: &str) -> serde_json::Value {
        self.driver
            .execute(script, Vec::new())
            .await
            .map(|r| r.json().clone())
            .unwrap_or_default()
    }

    /// Evaluate async JS — the script calls `arguments[args.len()-1]`
    /// (the done callback) with the result.
    pub async fn eval_async(&self, script: &str) -> serde_json::Value {
        self.driver
            .execute_async(script, Vec::new())
            .await
            .map(|r| r.json().clone())
            .unwrap_or_default()
    }

    /// First *displayed* element matching `css` — pages render hidden
    /// duplicates (desktop sidebar vs mobile drawer topbar) and the
    /// first DOM match may be the invisible one.
    pub async fn visible_elem(&self, css: &str, deadline: Instant) -> Option<WebElement> {
        loop {
            if let Ok(els) = self.driver.find_all(By::Css(css)).await {
                for el in els {
                    if el.is_displayed().await.unwrap_or(false) {
                        return Some(el);
                    }
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Quit the session + kill chromedriver.
    pub async fn shutdown(mut self) {
        let _ = self.driver.clone().quit().await;
        self.cleanup();
    }

    /// Kill chromedriver + any orphaned chrome children. A panicking
    /// test's Drop never ran `driver.quit`, leaving chrome reparented
    /// to init — and its inherited fd kept `/home/luis/.cargo.lock`
    /// held hostage. Match on the unique profile dir.
    fn cleanup(&mut self) {
        let _ = self.proc.kill();
        let _ = self.proc.wait();
        let pattern = format!("user-data-dir={}", self.profile.path().display());
        let _ = Command::new("pkill")
            .args(["-f", &pattern])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_file(workspace_root().join("target/.e2e-browser.lock"));
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.cleanup();
    }
}
