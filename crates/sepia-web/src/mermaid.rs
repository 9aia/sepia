//! Mermaid diagram rendering — the ```mermaid fence body is rendered
//! client-side by the vendored UMD bundle (`/vendor/mermaid.min.js`,
//! pinned mermaid@11.12.0 in `sepia-hub`). There is no pure-Rust
//! mermaid renderer, and the ~2.6 MiB lib is far too heavy to fold into
//! the wasm bundle, so it's lazy-loaded on the first diagram:
//!
//! 1. `render` injects `<script src="/vendor/mermaid.min.js">` once
//!    (the shared `LOADER` promise dedupes concurrent blocks and
//!    memoizes a failed load — no retry storm).
//! 2. `mermaid.initialize` runs once (`startOnLoad: false`,
//!    `securityLevel: "strict"` — labels are sanitized by the bundled
//!    DOMPurify and click handlers are disabled, so untrusted
//!    transcript text can't smuggle script through a diagram),
//!    `suppressErrorRendering` keeps the "syntax error" bomb element
//!    off the page, theme follows the `.light`/dark document class.
//! 3. `mermaid.parse` validates the source first, then
//!    `mermaid.render` produces the SVG written into the block's
//!    target div; the source `<pre>` hides behind `hidden`.
//!
//! Any failure — missing asset, parse error, layout error — leaves
//! the source code block visible (the pre-render SSR markup), which is
//! exactly what SSR and no-JS clients see anyway: graceful fallback.
//!
//! The whole file is behavior, not markup — SSR/hydrate emit
//! identical nodes and this module only mutates *inside* elements
//! Leptos already rendered.

use leptos::prelude::*;

/// Render `code` as a mermaid diagram into `target`, hiding `source`
/// on success. No-op off-wasm — SSR never runs the mounting Effect
/// that calls this, but the symbol must exist so the markup shape
/// stays feature-agnostic (the cfg-never-gates-markup rule).
#[cfg(not(feature = "hydrate"))]
pub fn render(
    _target: NodeRef<leptos::html::Div>,
    _source: NodeRef<leptos::html::Div>,
    _code: String,
) {
}

/// Hydrate implementation — see the module docs for the load/render
/// pipeline. Failures warn once and leave the fallback markup.
#[cfg(feature = "hydrate")]
pub fn render(
    target: NodeRef<leptos::html::Div>,
    source: NodeRef<leptos::html::Div>,
    code: String,
) {
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = render_inner(target, source, &code).await {
            let msg = format!("mermaid: {e:?}");
            web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&msg));
        }
    });
}

#[cfg(feature = "hydrate")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::{AtomicU32, Ordering};

    use wasm_bindgen::JsCast;
    use wasm_bindgen::JsValue;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::prelude::wasm_bindgen;

    #[wasm_bindgen]
    extern "C" {
        /// `window.mermaid` — the UMD global.
        pub type MermaidApi;

        #[wasm_bindgen(method, js_name = initialize)]
        pub fn initialize(this: &MermaidApi, config: &JsValue);

        /// Syntax-check diagram text; the returned promise rejects on
        /// invalid input without touching the DOM.
        #[wasm_bindgen(method, js_name = parse)]
        pub fn parse(this: &MermaidApi, text: &str) -> js_sys::Promise;

        /// Render to SVG — resolves `{svg, bindFunctions}`. We take
        /// `svg` only: `bindFunctions` wires `click` callbacks, which
        /// `securityLevel: "strict"` disables anyway.
        #[wasm_bindgen(method, js_name = render)]
        pub fn render(this: &MermaidApi, id: &str, text: &str) -> js_sys::Promise;
    }

    thread_local! {
        /// The one `<script>` load — `Some` once injected, resolved or
        /// rejected. A rejected promise is kept: the asset is either
        /// there or it isn't; retrying per block just spams 404s.
        static LOADER: RefCell<Option<js_sys::Promise>> = const { RefCell::new(None) };
        static INITIALIZED: Cell<bool> = const { Cell::new(false) };
    }

    /// Unique ids for `mermaid.render` — it names its scratch SVG
    /// after the id, so concurrent blocks must not collide.
    static NEXT_ID: AtomicU32 = AtomicU32::new(0);

    /// `window.mermaid`, when the bundle has landed.
    pub fn mermaid_global() -> Result<MermaidApi, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        js_sys::Reflect::get(&window, &JsValue::from_str("mermaid"))?
            .dyn_into::<MermaidApi>()
            .map_err(|_| JsValue::from_str("window.mermaid missing"))
    }

    /// Resolve once `/vendor/mermaid.min.js` has executed — injecting
    /// the script tag on first use. Deduped via [`LOADER`].
    pub fn ensure_loaded() -> Result<js_sys::Promise, JsValue> {
        LOADER.with(|slot| {
            if let Some(p) = slot.borrow().as_ref() {
                return Ok(p.clone());
            }
            if mermaid_global().is_ok() {
                // Already on the page (e.g. a warm SW-delivered script
                // tag from an earlier session).
                return Ok(js_sys::Promise::resolve(&JsValue::UNDEFINED));
            }
            let document = web_sys::window()
                .and_then(|w| w.document())
                .ok_or_else(|| JsValue::from_str("no document"))?;
            let script = document
                .create_element("script")?
                .unchecked_into::<web_sys::HtmlScriptElement>();
            // Handlers attach before the append — the executor runs
            // synchronously inside `Promise::new`.
            let promise = js_sys::Promise::new(&mut |resolve, reject| {
                // `once_into_js` intentionally leaks the tiny closure
                // (once per page load) — `Closure::once` handles
                // self-free on invoke but the handle must outlive the
                // script tag's event, which nothing else retains.
                let onload = Closure::once_into_js(move || {
                    let _ = resolve.call0(&JsValue::NULL);
                });
                let onerror = Closure::once_into_js(move |e: JsValue| {
                    let _ = reject.call1(&JsValue::NULL, &e);
                });
                script.set_onload(Some(onload.unchecked_ref::<js_sys::Function>()));
                script.set_onerror(Some(onerror.unchecked_ref::<js_sys::Function>()));
            });
            script.set_src("/vendor/mermaid.min.js");
            let parent = document
                .head()
                .map(JsCast::unchecked_into::<web_sys::Element>)
                .or_else(|| document.document_element())
                .ok_or_else(|| JsValue::from_str("no head"))?;
            parent.append_child(&script)?;
            *slot.borrow_mut() = Some(promise.clone());
            Ok(promise)
        })
    }

    /// `dark` unless the document carries the `.light` class — the
    /// same class the theme system toggles. Read once at init;
    /// diagrams don't re-theme live (a reload picks it up).
    fn theme() -> &'static str {
        let light = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
            .is_some_and(|el| el.class_list().contains("light"));
        if light { "default" } else { "dark" }
    }

    /// One-time `mermaid.initialize`. `securityLevel: "strict"` is the
    /// XSS posture: diagram text is untrusted transcript content.
    pub fn init_once(mermaid: &MermaidApi) {
        if INITIALIZED.with(Cell::get) {
            return;
        }
        let config = js_sys::Object::new();
        let set = |k: &str, v: JsValue| {
            let _ = js_sys::Reflect::set(&config, &JsValue::from_str(k), &v);
        };
        set("startOnLoad", false.into());
        set("securityLevel", "strict".into());
        set("suppressErrorRendering", true.into());
        set("theme", theme().into());
        set("fontFamily", "ui-monospace, monospace".into());
        mermaid.initialize(&config.into());
        INITIALIZED.with(|c| c.set(true));
    }

    pub fn next_id() -> String {
        format!("sepia-mmd-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// The async pipeline behind [`render`]: load → init → parse → render
/// → swap the SVG in and hide the source. Any `Err` leaves the
/// fallback markup untouched.
#[cfg(feature = "hydrate")]
async fn render_inner(
    target: NodeRef<leptos::html::Div>,
    source: NodeRef<leptos::html::Div>,
    code: &str,
) -> Result<(), wasm_bindgen::JsValue> {
    use imp::{ensure_loaded, init_once, mermaid_global, next_id};
    use wasm_bindgen::JsValue;
    use wasm_bindgen_futures::JsFuture;

    JsFuture::from(ensure_loaded()?).await?;
    let mermaid = mermaid_global()?;
    init_once(&mermaid);
    // Parse first — a rejected parse keeps the source block and never
    // creates mermaid's error element.
    JsFuture::from(mermaid.parse(code)).await?;
    let result = JsFuture::from(mermaid.render(&next_id(), code)).await?;
    let svg = js_sys::Reflect::get(&result, &JsValue::from_str("svg"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("render returned no svg"))?;
    if let Some(el) = target.get() {
        el.set_inner_html(&svg);
        el.remove_attribute("hidden")?;
    }
    if let Some(el) = source.get() {
        el.set_attribute("hidden", "")?;
    }
    Ok(())
}
