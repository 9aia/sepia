#![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

//! Browser e2e suite — real `sepia-node` + `sepia-hub` + headless
//! Chrome driven over WebDriver (thirtyfour).
//!
//! `ssr_integrity` needs only the workspace binaries and always runs.
//! The browser suites are gated: run with
//!
//! ```bash
//! SEPIA_BROWSER_E2E=1 cargo nextest run -p sepia-hub --test e2e
//! ```
//!
//! They need a chromedriver binary (`SEPIA_CHROMEDRIVER`, else
//! `target/webdriver/chromedriver`, else `chromedriver` on PATH) and a
//! Chrome/Chromium binary (`CHROME_BIN`, else PATH/common-path probe).

mod harness;
mod hydration;
mod journey;
mod sessions;
mod shell;
mod ssr_integrity;
