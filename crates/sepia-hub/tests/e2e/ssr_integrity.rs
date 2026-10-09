//! SSR stream integrity — no browser needed. A mid-stream panic
//! (the `SendWrapper` owner-cleanup bug) truncates the chunked body:
//! `read_to_string` errors or the tail `</html>` never arrives.

use crate::harness::{self, E2eEnv};

#[tokio::test]
async fn ssr_responses_are_complete_on_every_route() {
    let env = E2eEnv::spawn().await;
    for path in harness::ROUTES {
        let body = env.get(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        // leptos_axum out-of-order streaming appends resolved-resource
        // <script> fragments after </html>, so the tail is scripts, not
        // the close tag. `env.get` already errors on a mid-stream abort
        // (panic kills the chunked stream); here we only need the tail
        // marker present and the shell rendered.
        assert!(
            body.contains("</html>") && body.contains("__INCOMPLETE_CHUNKS"),
            "{path}: SSR body truncated — no </html>/chunk-finalizer in {} bytes",
            body.len()
        );
        assert!(
            body.contains(r#"class="flex h-dvh"#),
            "{path}: app shell missing from SSR output"
        );
    }
    let (hub_err, node_err) = env.shutdown();
    assert!(
        !hub_err.contains("panicked"),
        "hub worker panicked while serving:\n{hub_err}"
    );
    assert!(
        !node_err.contains("panicked"),
        "node panicked while serving:\n{node_err}"
    );
}

#[tokio::test]
async fn ssr_emits_content_hashed_bundle_urls() {
    let env = E2eEnv::spawn().await;
    let body = env.get("/").unwrap();
    // `sepia_web_<hash>.{js,bg.wasm}` — fixed-name bundle URLs would
    // let a stale cache hydrate old wasm against fresh markup.
    assert!(
        body.contains("/pkg/sepia_web_"),
        "SSR should reference a content-hashed bundle:\n{body}"
    );
    env.shutdown();
}

#[tokio::test]
async fn static_assets_are_fresh() {
    let env = E2eEnv::spawn().await;
    // sw.js must be no-cache: the browser's SW update check honors
    // HTTP cache, and a stale worker perpetuates itself.
    let resp = env
        .http
        .head(format!("{}sw.js", env.hub_url))
        .call()
        .unwrap();
    let cc = resp
        .headers()
        .get("cache-control")
        .map(|v| v.to_str().unwrap_or(""))
        .unwrap_or("")
        .to_string();
    assert!(
        cc.contains("no-cache"),
        "sw.js must be served no-cache, got: {cc:?}"
    );
    env.shutdown();
}
