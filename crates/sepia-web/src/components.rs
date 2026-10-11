//! shadcn/rust-ui style components on `leptos_ui` (`clx!`, `variants!`,
//! `tw_merge`). All components render deterministic markup — SSR and
//! hydration must agree, so nothing here branches on `Resource` data
//! outside a `Suspend` boundary.

use leptos::prelude::*;
use leptos_ui::{clx, variants, void};

variants! {
    Button {
        // The old app's shadcn recipe — pill shape, ring focus, press
        // dip, translucent destructive.
        base: "inline-flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-4xl border border-transparent bg-clip-padding text-sm font-medium transition-all select-none cursor-pointer focus-visible:ring-2 focus-visible:ring-ring active:not-aria-[haspopup]:translate-y-px disabled:pointer-events-none disabled:opacity-50",
        variants: {
            variant: {
                Default: "bg-primary text-primary-foreground hover:bg-primary/80",
                Secondary: "bg-secondary text-secondary-foreground hover:bg-[color-mix(in_oklch,var(--secondary),var(--foreground)_5%)]",
                Ghost: "hover:bg-muted hover:text-foreground dark:hover:bg-muted/50",
                Outline: "border-border bg-background hover:bg-muted hover:text-foreground dark:bg-transparent dark:hover:bg-input/30",
                Destructive: "bg-destructive/10 text-destructive hover:bg-destructive/20 dark:bg-destructive/20 dark:hover:bg-destructive/30",
                Link: "text-primary underline-offset-4 hover:underline",
            },
            size: {
                Default: "h-9 px-3",
                Sm: "h-8 gap-1 px-3",
                Lg: "h-10 gap-1.5 px-4",
                Icon: "size-9",
            }
        }
    }
}

#[component]
pub fn Button(
    #[prop(optional)] variant: ButtonVariant,
    #[prop(optional)] size: ButtonSize,
    #[prop(into, optional)] class: String,
    #[prop(optional, into)] button_type: String,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional)] on_click: Option<Box<dyn Fn() + Send + Sync + 'static>>,
    children: Children,
) -> impl IntoView {
    let class = tw_merge::tw_merge!(ButtonClass { variant, size }.to_class(), class);
    view! {
        <button
            type=move || if button_type.is_empty() { "button".to_string() } else { button_type.clone() }
            class=class
            disabled=move || disabled.get()
            on:click=move |ev| {
                if let Some(cb) = &on_click {
                    ev.prevent_default();
                    cb();
                }
            }
        >
            {children()}
        </button>
    }
}

void! {
    Input,
    input,
    // The old app's field recipe — filled, pill-shaped, ring focus.
    // `text-base md:text-sm` keeps iOS from zooming on focus.
    "flex h-9 w-full min-w-0 rounded-3xl border border-transparent bg-input/50 px-3 py-1 text-base transition-[color,box-shadow,background-color] placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 md:text-sm"
}

/// `<textarea>` / `<select>` need children or multiple attrs — apply
/// these class strings on the native elements instead of components.
pub const TEXTAREA_CLASS: &str = "flex field-sizing-content min-h-16 w-full resize-none rounded-2xl border border-transparent bg-input/50 px-3 py-3 text-base transition-[color,box-shadow,background-color] placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 md:text-sm";

pub const SELECT_CLASS: &str = "flex h-9 w-full appearance-none items-center justify-between gap-1.5 rounded-3xl border border-transparent bg-input/50 px-3 py-2 text-sm whitespace-nowrap transition-[color,box-shadow,background-color] focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50";

variants! {
    Badge {
        // The old app's shadcn badge — fixed-height pill, plain case.
        base: "inline-flex h-5 w-fit shrink-0 items-center justify-center gap-1 rounded-3xl border px-2 py-0.5 text-xs font-medium whitespace-nowrap",
        variants: {
            variant: {
                Default: "border-transparent bg-primary/15 text-primary",
                Secondary: "border-transparent bg-secondary text-secondary-foreground",
                Info: "border-transparent bg-info/15 text-info",
                Success: "border-transparent bg-success/15 text-success",
                Warning: "border-transparent bg-warning/15 text-warning",
                Destructive: "border-transparent bg-destructive/15 text-destructive",
                Muted: "border-transparent bg-muted text-muted-foreground",
                Outline: "border-border text-muted-foreground",
            },
            size: {
                Default: "px-2 py-0.5",
                Sm: "px-1.5 py-px",
            }
        }
    }
}

#[component]
pub fn Badge(
    #[prop(optional)] variant: BadgeVariant,
    #[prop(into, optional)] class: String,
    children: Children,
) -> impl IntoView {
    let class = tw_merge::tw_merge!(
        BadgeClass {
            variant,
            size: BadgeSize::Default
        }
        .to_class(),
        class
    );
    view! { <span class=class>{children()}</span> }
}

clx! { Card, div, "rounded-lg border bg-card text-card-foreground shadow-sm" }
clx! { CardHeader, div, "flex flex-col gap-1.5 p-5" }
clx! { CardTitle, h3, "font-semibold leading-none tracking-tight" }
clx! { CardDescription, p, "text-sm text-muted-foreground" }
clx! { CardContent, div, "p-5 pt-0" }
clx! { CardFooter, div, "flex items-center p-5 pt-0" }

void! { Separator, hr, "shrink-0 border-none bg-border h-px w-full" }

void! { Skeleton, div, "animate-pulse rounded-md bg-muted" }

clx! {
    PageHead,
    header,
    "mb-6 flex items-end justify-between gap-4"
}

clx! {
    PageTitle,
    h1,
    "text-xl font-semibold tracking-tight"
}

clx! {
    PageDescription,
    p,
    "text-sm text-muted-foreground"
}

/// Native `<details>` dropdown — works without wasm, hydrates cleanly,
/// closes on outside click via a fixed transparent backdrop. `label`
/// renders a labeled pill trigger (the row-actions menu); `icon` swaps
/// in an icon-only ghost trigger instead (the old app's ⋯ affordance).
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
pub fn Dropdown(
    #[prop(into, optional)] label: String,
    /// Lucide name for an icon-only trigger — wins over `label`.
    #[prop(into, optional)]
    icon: String,
    /// Accessible name + tooltip on the icon-only trigger.
    #[prop(into, optional)]
    aria_label: String,
    /// Menu panel width (`w-44` default).
    #[prop(into, optional)]
    menu_class: String,
    #[prop(optional)] class: String,
    children: Children,
) -> impl IntoView {
    view! {
        <details class=tw_merge::tw_merge!("group relative", class)>
            <summary
                class="list-none cursor-pointer select-none [&::-webkit-details-marker]:hidden"
                aria-label=(!icon.is_empty()).then(|| aria_label.clone())
                title=(!icon.is_empty()).then(|| aria_label.clone())
            >
                {if icon.is_empty() {
                    view! {
                        <span class=ButtonClass {
                            variant: ButtonVariant::Secondary,
                            size: ButtonSize::Sm,
                        }.to_class()>
                            {label}
                            <icons::Icon
                                name="chevron-down"
                                class="size-3 opacity-60 transition-transform group-open:rotate-180"
                            />
                        </span>
                    }
                    .into_any()
                } else {
                    view! {
                        <span class=ButtonClass {
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Icon,
                        }.to_class()>
                            <icons::Icon name=icon.clone()/>
                        </span>
                    }
                    .into_any()
                }}
            </summary>
            // open: sibling backdrop swallows the outside click
            <span class="fixed inset-0 z-40 hidden cursor-default group-open:block" onclick="this.parentElement.removeAttribute('open')"></span>
            <div class=tw_merge::tw_merge!("absolute right-0 z-50 mt-1 w-44 rounded-3xl bg-popover p-1.5 text-popover-foreground shadow-lg ring-1 ring-border", menu_class)>
                {children()}
            </div>
        </details>
    }
}

/// Menu item inside a `Dropdown`. `icon` is a lucide name rendered at
/// the row's start (the old app's items all led with a glyph).
#[component]
pub fn MenuItem(
    #[prop(into)] label: String,
    /// Lucide name for the leading glyph (empty → text-only row).
    #[prop(into, optional)]
    icon: String,
    #[prop(optional)] destructive: bool,
    /// Keeps the menu open after the click (toggles like project
    /// membership — the old app's `closeOnClick={false}`).
    #[prop(optional)]
    keep_open: bool,
    #[prop(optional)] on_click: Option<Box<dyn Fn() + Send + Sync + 'static>>,
) -> impl IntoView {
    let cls = if destructive {
        "flex w-full items-center gap-2.5 rounded-2xl px-3 py-2 text-sm font-medium text-destructive hover:bg-destructive/10 cursor-pointer"
    } else {
        "flex w-full items-center gap-2.5 rounded-2xl px-3 py-2 text-sm font-medium hover:bg-accent cursor-pointer"
    };
    let close = (!keep_open).then_some("this.closest('details').removeAttribute('open')");
    view! {
        <button type="button" class=cls onclick=close
            on:click=move |ev| {
                if let Some(cb) = &on_click {
                    ev.prevent_default();
                    cb();
                }
            }
        >
            {(!icon.is_empty()).then(|| {
                view! { <icons::Icon name=icon class="size-4 text-muted-foreground"/> }
            })}
            {label}
        </button>
    }
}

// The thin rule between menu item groups (the old app's
// `DropdownMenuSeparator`).
void! { MenuSeparator, hr, "my-1 h-px border-none bg-border" }

/// Confirm dialog — signal-driven modal. The markup is always
/// rendered (SSR/hydrate agree on the DOM); visibility is a class
/// toggle, so no `Show` `Fn` gymnastics.
///
/// a11y: `role="dialog"` + `aria-modal`; `id` anchors
/// `aria-labelledby` (pass a unique stem per dialog — several
/// instances share titles like "Delete this session?"); without `id`
/// the title doubles as `aria-label`. Escape closes, the first
/// focusable takes focus on open, Tab/Shift+Tab wrap inside the
/// dialog, and the previously focused element regains focus on close
/// (`track_overlay_focus`).
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
pub fn ConfirmDialog(
    open: RwSignal<bool>,
    #[prop(into)] title: String,
    #[prop(into)] body: String,
    #[prop(into, optional)] confirm_label: String,
    /// Stem for the heading's id — enables `aria-labelledby`.
    #[prop(into, optional)]
    id: String,
    #[prop(optional)] destructive: bool,
    on_confirm: impl Fn() + Send + Sync + 'static,
) -> impl IntoView {
    let on_confirm = std::sync::Arc::new(on_confirm);
    let confirm_label = if confirm_label.is_empty() {
        "Confirm".to_string()
    } else {
        confirm_label
    };
    let panel = NodeRef::<leptos::html::Div>::new();
    track_overlay_focus(open, panel);
    let title_id = if id.is_empty() {
        None
    } else {
        Some(format!("{id}-title"))
    };
    let labelledby = title_id.clone();
    // Fallback label when no `id` stem was passed.
    let aria_label = labelledby.is_none().then(|| title.clone());
    view! {
        <div
            class=move || {
                if open.get() {
                    "fixed inset-0 z-50 grid place-items-center bg-black/30 p-4 supports-backdrop-filter:backdrop-blur-sm"
                } else {
                    "hidden"
                }
            }
            on:click=move |_| open.set(false)
        >
            <div
                node_ref=panel
                class="w-full max-w-md rounded-4xl bg-popover p-6 text-popover-foreground shadow-xl ring-1 ring-border focus:outline-none"
                role="dialog"
                aria-modal="true"
                aria-labelledby=move || labelledby.clone()
                aria-label=move || aria_label.clone()
                tabindex="-1"
                on:click=|ev| ev.stop_propagation()
                on:keydown=move |ev| {
                    if ev.key() == "Escape" {
                        open.set(false);
                    }
                }
            >
                <h3 id=move || title_id.clone() class="text-base font-semibold">{title}</h3>
                <p class="mt-2 text-sm text-muted-foreground">{body}</p>
                <div class="mt-5 flex justify-end gap-2">
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        on_click=Box::new(move || open.set(false))
                    >
                        "Cancel"
                    </Button>
                    <Button
                        variant=if destructive { ButtonVariant::Destructive } else { ButtonVariant::Default }
                        size=ButtonSize::Sm
                        on_click=Box::new(move || {
                            on_confirm();
                            open.set(false);
                        })
                    >
                        {confirm_label}
                    </Button>
                </div>
            </div>
        </div>
    }
}

/// Inline error banner — warning glyph + message + an optional retry
/// action (`on_retry` refetches the owning resource where one exists).
/// One component for every failed load or rejected mutation; the old
/// per-page raw `<p class=ERROR_BOX>` copies are gone.
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
pub fn ErrorBanner(
    #[prop(into)] message: String,
    #[prop(optional)] on_retry: Option<Box<dyn Fn() + Send + Sync + 'static>>,
    #[prop(into, optional)] class: String,
) -> impl IntoView {
    view! {
        <div
            data-name="ErrorBanner"
            role="alert"
            class=tw_merge::tw_merge!(
                "flex items-center gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive",
                class
            )
        >
            <icons::Icon name="triangle-alert"/>
            <span class="min-w-0 flex-1 break-words">{message}</span>
            {on_retry.map(|cb| {
                view! {
                    <Button
                        variant=ButtonVariant::Outline
                        size=ButtonSize::Sm
                        class="shrink-0 border-destructive/40 text-destructive hover:bg-destructive/10"
                        on_click=cb
                    >
                        "Retry"
                    </Button>
                }
            })}
        </div>
    }
}

/// Screen-reader-only wrapper — hidden visually, still announced.
/// Backs the toast live region.
#[component]
pub fn VisuallyHidden(children: Children) -> impl IntoView {
    view! { <span class="sr-only">{children()}</span> }
}

/// Clipboard write — browser-only (`navigator.clipboard`); the SSR
/// stub keeps `on:click` handlers compiling. Shared by the code-fence
/// copy button and the per-message copy action.
#[cfg(feature = "hydrate")]
pub(crate) fn copy_to_clipboard(text: &str) {
    let _ = window().navigator().clipboard().write_text(text);
}

/// SSR counterpart — no DOM, nothing to write.
#[cfg(not(feature = "hydrate"))]
pub(crate) fn copy_to_clipboard(_: &str) {}

/// Focus management for signal-driven overlay panels — initial focus,
/// a full Tab trap, and return-focus (`leptos-use` 0.19 has no
/// `use_focus_trap`, so the trap is hand-rolled in `overlay` below).
/// `panel` needs `tabindex="-1"` as the fallback focus target: on
/// open the first focusable (or a `[data-autofocus]` mark) takes
/// focus, Tab/Shift+Tab wrap inside the panel while it's the topmost
/// overlay, and whatever was focused before regains it on close.
/// Escape stays each overlay's own keydown job. Hydrate-only: SSR
/// never runs effects and has no DOM.
#[cfg(feature = "hydrate")]
pub(crate) fn track_overlay_focus(open: RwSignal<bool>, panel: NodeRef<leptos::html::Div>) {
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    overlay::ensure_tab_listener();
    let id = overlay::next_id();
    let prev = Rc::new(RefCell::new(None::<web_sys::HtmlElement>));
    Effect::new(move |_| {
        if open.get() {
            if let Some(el) = document()
                .active_element()
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
            {
                *prev.borrow_mut() = Some(el);
            }
            if let Some(el) = panel.get() {
                overlay::push(id, el.clone().unchecked_into());
                overlay::focus_initial(&el);
            }
        } else {
            overlay::pop(id);
            if let Some(el) = prev.borrow_mut().take() {
                let _ = el.focus();
            }
        }
    });
    // Unmounted while open — leave the stack clean.
    on_cleanup(move || overlay::pop(id));
}

/// SSR/no-DOM counterpart — effects never run, there's nothing to
/// focus. Keeps the call site unconditional (cfg never gates markup).
#[cfg(not(feature = "hydrate"))]
pub(crate) fn track_overlay_focus(_open: RwSignal<bool>, _panel: NodeRef<leptos::html::Div>) {}

/// The DOM half of the overlay Tab trap — a stack of open panels
/// (push order = open order) plus one capture-phase document keydown
/// listener that wraps Tab/Shift+Tab inside the TOP entry only, which
/// is what keeps a `ConfirmDialog` over a `Sheet` sane. Focusables
/// are re-queried per press so content mounted after open (palette
/// results, lazy lists) joins the cycle; the wrap math is pure
/// (`sepia_web_core::focus`).
#[cfg(feature = "hydrate")]
mod overlay {
    use leptos::prelude::*;
    use sepia_web_core::focus::{self, TabDir};
    use std::cell::{Cell, RefCell};
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;

    /// One open overlay — `panel` is its `role=dialog` element.
    struct Entry {
        id: u64,
        panel: web_sys::HtmlElement,
    }

    thread_local! {
        /// Open overlays, bottom → top. Entries land on open (not
        /// mount) so stacking order matches what's on screen.
        static STACK: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
        static NEXT_ID: Cell<u64> = const { Cell::new(1) };
        /// The document listener is installed once and lives for the
        /// page's lifetime — with an empty stack it's a no-op.
        static LISTENER_UP: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) fn next_id() -> u64 {
        NEXT_ID.with(|n| {
            let id = n.get();
            n.set(id + 1);
            id
        })
    }

    /// Mark `id` open — retain-then-push keeps rapid toggles
    /// idempotent.
    pub(super) fn push(id: u64, panel: web_sys::HtmlElement) {
        STACK.with(|s| {
            let mut s = s.borrow_mut();
            s.retain(|e| e.id != id);
            s.push(Entry { id, panel });
        });
    }

    pub(super) fn pop(id: u64) {
        STACK.with(|s| s.borrow_mut().retain(|e| e.id != id));
    }

    /// The topmost open overlay's panel — the only one the trap
    /// wraps Tab inside.
    fn top() -> Option<web_sys::HtmlElement> {
        STACK.with(|s| s.borrow().last().map(|e| e.panel.clone()))
    }

    /// Live focusable elements inside `panel`, in DOM order. The
    /// `offset_*` box check filters `display:none`/`hidden` subtrees
    /// the selector can't express.
    fn focusables(panel: &web_sys::Element) -> Vec<web_sys::HtmlElement> {
        let Ok(list) = panel.query_selector_all(focus::FOCUSABLE_SELECTOR) else {
            return Vec::new();
        };
        (0..list.length())
            .filter_map(|i| list.item(i))
            .filter_map(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
            .filter(|el| el.offset_width() > 0 || el.offset_height() > 0)
            .collect()
    }

    /// Focus on open — a `[data-autofocus]` mark wins, else the first
    /// focusable, else the panel itself. Deferred a tick so the
    /// `inert`/`aria-hidden` attribute effects land first: `focus()`
    /// inside a still-inert subtree is a silent no-op.
    pub(super) fn focus_initial(panel: &web_sys::HtmlElement) {
        let panel = panel.clone();
        let cb = Closure::<dyn FnMut()>::once(move || {
            let items = focusables(&panel);
            let auto = panel
                .query_selector("[data-autofocus]")
                .ok()
                .flatten()
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
                .and_then(|a| {
                    items
                        .iter()
                        .position(|el| a.is_same_node(Some(el.unchecked_ref())))
                });
            let target =
                focus::initial_target(auto, items.len()).map_or(panel, |i| items[i].clone());
            let _ = target.focus();
        });
        let _ = window()
            .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), 0);
        cb.forget();
    }

    /// One document-level capture listener shared by every overlay —
    /// it wraps Tab inside the stack's top entry only. Capture phase
    /// so the wrap lands before any inner handler moves focus.
    pub(super) fn ensure_tab_listener() {
        LISTENER_UP.with(|up| {
            if up.replace(true) {
                return;
            }
            let cb = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(|ev| on_keydown(&ev));
            let opts = web_sys::AddEventListenerOptions::new();
            opts.set_capture(true);
            let _ = document().add_event_listener_with_callback_and_add_event_listener_options(
                "keydown",
                cb.as_ref().unchecked_ref(),
                &opts,
            );
            cb.forget();
        });
    }

    fn on_keydown(ev: &web_sys::KeyboardEvent) {
        if ev.key() != "Tab" {
            return;
        }
        let Some(panel) = top() else {
            return;
        };
        let items = focusables(&panel);
        let current = document()
            .active_element()
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
            .and_then(|a| {
                items
                    .iter()
                    .position(|el| a.is_same_node(Some(el.unchecked_ref())))
            });
        let dir = if ev.shift_key() {
            TabDir::Backward
        } else {
            TabDir::Forward
        };
        // We move focus ourselves every press — deterministic wrap no
        // matter where focus sat (panel, list edge, page behind).
        ev.prevent_default();
        let target =
            focus::tab_target(current, items.len(), dir).map_or(panel, |i| items[i].clone());
        let _ = target.focus();
    }
}

/// Lucide icons (`lucide-leptos` — inline SVG components, so SSR and
/// hydrate emit identical markup and no JS is involved). The crate's
/// components take `size`/`color` props but no `class`, so `Icon`
/// wraps the glyph in a span that carries the Tailwind box: `size-*`
/// shrinks the span and `[&>svg]:size-full` scales the svg inside,
/// `text-*` colors flow through `stroke="currentColor"`.
pub mod icons {
    use leptos::prelude::*;

    /// A stroke icon. `name` is the lucide kebab-case id; known names:
    /// `message-square` (default), `bot`, `folder`, `server`,
    /// `settings`, `pin`, `plus`, `panel-right`, `panel-left-close`,
    /// `x`, `ellipsis-vertical`, `menu`, `arrow-left`, `arrow-down`,
    /// `chevron-right`, `chevron-down`, `corner-down-right`,
    /// `cloud-off`, `file-question-mark`, `message-square-dashed`,
    /// `send`, `paperclip`, `lock`, `inbox`, `pencil`, `copy`,
    /// `check`, `download`, `upload`, `archive`, `info`, `plug`,
    /// `unplug`, `square`, `loader-circle`, `trash`, `folder-open`,
    /// `globe`, `octagon-x`, `file`, `monitor`, `rotate-ccw`,
    /// `keyboard`, `search`, `sliders-horizontal`, `arrow-up-down`,
    /// `clock`, `circle-dot`, `chevron-left`.
    #[component]
    #[allow(clippy::needless_pass_by_value)] // component props are owned
    pub fn Icon(
        #[prop(into)] name: String,
        #[prop(into, optional)] class: String,
    ) -> impl IntoView {
        let class = tw_merge::tw_merge!(
            "inline-flex size-4 shrink-0 items-center justify-center [&>svg]:size-full",
            class
        );
        view! {
            <span class=class aria-hidden="true">
                {match name.as_str() {
                    "bot" => view! { <lucide_leptos::Bot/> }.into_any(),
                    "folder" => view! { <lucide_leptos::Folder/> }.into_any(),
                    "server" => view! { <lucide_leptos::Server/> }.into_any(),
                    "settings" => view! { <lucide_leptos::Settings/> }.into_any(),
                    "pin" => view! { <lucide_leptos::Pin/> }.into_any(),
                    "plus" => view! { <lucide_leptos::Plus/> }.into_any(),
                    "panel-right" => view! { <lucide_leptos::PanelRight/> }.into_any(),
                    "panel-left-close" => {
                        view! { <lucide_leptos::PanelLeftClose/> }.into_any()
                    }
                    "triangle-alert" => {
                        view! { <lucide_leptos::TriangleAlert/> }.into_any()
                    }
                    "x" => view! { <lucide_leptos::X/> }.into_any(),
                    "ellipsis-vertical" => {
                        view! { <lucide_leptos::EllipsisVertical/> }.into_any()
                    }
                    "menu" => view! { <lucide_leptos::Menu/> }.into_any(),
                    "arrow-left" => view! { <lucide_leptos::ArrowLeft/> }.into_any(),
                    "arrow-down" => view! { <lucide_leptos::ArrowDown/> }.into_any(),
                    "chevron-right" => {
                        view! { <lucide_leptos::ChevronRight/> }.into_any()
                    }
                    "chevron-down" => {
                        view! { <lucide_leptos::ChevronDown/> }.into_any()
                    }
                    "corner-down-right" => {
                        view! { <lucide_leptos::CornerDownRight/> }.into_any()
                    }
                    "cloud-off" => view! { <lucide_leptos::CloudOff/> }.into_any(),
                    "file-question-mark" => {
                        view! { <lucide_leptos::FileQuestionMark/> }.into_any()
                    }
                    "message-square-dashed" => {
                        view! { <lucide_leptos::MessageSquareDashed/> }.into_any()
                    }
                    "send" => view! { <lucide_leptos::Send/> }.into_any(),
                    "paperclip" => view! { <lucide_leptos::Paperclip/> }.into_any(),
                    "lock" => view! { <lucide_leptos::Lock/> }.into_any(),
                    "inbox" => view! { <lucide_leptos::Inbox/> }.into_any(),
                    "pencil" => view! { <lucide_leptos::Pencil/> }.into_any(),
                    "copy" => view! { <lucide_leptos::Copy/> }.into_any(),
                    "check" => view! { <lucide_leptos::Check/> }.into_any(),
                    "download" => view! { <lucide_leptos::Download/> }.into_any(),
                    "upload" => view! { <lucide_leptos::Upload/> }.into_any(),
                    "archive" => view! { <lucide_leptos::Archive/> }.into_any(),
                    "info" => view! { <lucide_leptos::Info/> }.into_any(),
                    "plug" => view! { <lucide_leptos::Plug/> }.into_any(),
                    "unplug" => view! { <lucide_leptos::Unplug/> }.into_any(),
                    "square" => view! { <lucide_leptos::Square/> }.into_any(),
                    "loader-circle" => {
                        view! { <lucide_leptos::LoaderCircle/> }.into_any()
                    }
                    "trash" => view! { <lucide_leptos::Trash/> }.into_any(),
                    "folder-open" => {
                        view! { <lucide_leptos::FolderOpen/> }.into_any()
                    }
                    "globe" => view! { <lucide_leptos::Globe/> }.into_any(),
                    "octagon-x" => view! { <lucide_leptos::OctagonX/> }.into_any(),
                    "file" => view! { <lucide_leptos::File/> }.into_any(),
                    "monitor" => view! { <lucide_leptos::Monitor/> }.into_any(),
                    "rotate-ccw" => view! { <lucide_leptos::RotateCcw/> }.into_any(),
                    "keyboard" => view! { <lucide_leptos::Keyboard/> }.into_any(),
                    "search" => view! { <lucide_leptos::Search/> }.into_any(),
                    "sliders-horizontal" => {
                        view! { <lucide_leptos::SlidersHorizontal/> }.into_any()
                    }
                    "arrow-up-down" => {
                        view! { <lucide_leptos::ArrowUpDown/> }.into_any()
                    }
                    "clock" => view! { <lucide_leptos::Clock/> }.into_any(),
                    "circle-dot" => view! { <lucide_leptos::CircleDot/> }.into_any(),
                    "chevron-left" => {
                        view! { <lucide_leptos::ChevronLeft/> }.into_any()
                    }
                    _ => view! { <lucide_leptos::MessageSquare/> }.into_any(),
                }}
            </span>
        }
    }
}

pub mod toast {
    //! Thin wrapper over `leptos_toaster` (sonner-style) — call
    //! `provide_toaster()` in `App`, `use_toast()` for
    //! `.success()/.error()/.info()`, render `<Toaster/>` once.
    //! Dark theme is fixed to match the app's dark-first palette.

    use leptos::prelude::*;
    use leptos_toaster::{Theme, Toast, ToastId, ToastVariant, Toasts};

    /// Toast store — a `Toasts` context wrapper with our API. The
    /// `announce` signal mirrors the last toast text into the
    /// `Toaster`'s `aria-live` region (the sonner stack is visual-only).
    #[derive(Clone, Copy)]
    pub struct ToastStore {
        toasts: Toasts,
        announce: RwSignal<String>,
    }

    impl ToastStore {
        fn push(&self, variant: ToastVariant, msg: String) {
            self.announce.set(msg.clone());
            let toast_id = ToastId::new();
            self.toasts.toast(
                leptos::prelude::ViewFn::from(move || {
                    let title_msg = msg.clone();
                    view! {
                        <Toast
                            toast_id
                            variant=variant
                            theme=Theme::Dark
                            title=move || title_msg.clone()
                        />
                    }
                }),
                Some(toast_id),
                None,
            );
        }
        pub fn success(&self, msg: impl Into<String>) {
            self.push(ToastVariant::Success, msg.into());
        }
        pub fn error(&self, msg: impl Into<String>) {
            self.push(ToastVariant::Error, msg.into());
        }
        pub fn info(&self, msg: impl Into<String>) {
            self.push(ToastVariant::Info, msg.into());
        }
        /// Mutation feedback shorthand — `Ok` → success toast with
        /// `ok` (and the payload handed back), `Err` → error toast
        /// with the message. Pages wrap `spawn_local` mutation calls
        /// with this instead of hand-branching.
        pub fn outcome<T>(
            &self,
            res: Result<T, impl std::fmt::Display>,
            ok: impl Into<String>,
        ) -> Option<T> {
            match res {
                Ok(v) => {
                    self.success(ok);
                    Some(v)
                }
                Err(e) => {
                    self.error(e.to_string());
                    None
                }
            }
        }
    }

    pub fn provide_toaster() {
        provide_context(ToastStore {
            toasts: leptos_toaster::provide_toasts(),
            announce: RwSignal::new(String::new()),
        });
    }

    pub fn use_toast() -> ToastStore {
        expect_context::<ToastStore>()
    }

    /// Bottom-right sonner stack + a `role="status"` live region that
    /// announces every toast's text to screen readers (the visual
    /// toasts are `leptos_toaster`'s and carry no live semantics).
    /// Renders deterministically — empty on SSR and initial hydrate.
    #[component]
    pub fn Toaster() -> impl IntoView {
        let announce = use_toast().announce;
        view! {
            <leptos_toaster::Toaster/>
            <super::VisuallyHidden>
                <span role="status" aria-live="polite">
                    {move || announce.get()}
                </span>
            </super::VisuallyHidden>
        }
    }
}

/// Slide-over panel — the old app's base-ui `Drawer` recipe: the
/// panel floats `inset-2` off the viewport edge (rounded-4xl), slides
/// in over 450ms on the drawer's cubic-bezier, and the backdrop fades
/// on its own 450ms curve with a blur. Signal-driven; markup always
/// rendered so SSR/hydrate agree — visibility is a class transition.
///
/// a11y: `role="dialog"` + `aria-modal`, `aria-label` when `label` is
/// given, Escape closes, the first focusable takes focus on open,
/// Tab/Shift+Tab wrap inside the sheet while it's open, and focus
/// returns to the previously focused element on close
/// (`track_overlay_focus`).
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
pub fn Sheet(
    open: RwSignal<bool>,
    /// "left" | "right"
    #[prop(into, optional)]
    side: String,
    #[prop(into, optional)] class: String,
    /// Accessible name — `aria-label` on the dialog panel.
    #[prop(into, optional)]
    label: String,
    children: Children,
) -> impl IntoView {
    // Closed transform = panel width + the 8px inset + 2px of shadow
    // bleed (the old drawer's `--closed-transform`).
    let side_cls = if side == "left" {
        "left-2 -translate-x-[calc(100%+10px)] data-[open]:translate-x-0"
    } else {
        "right-2 translate-x-[calc(100%+10px)] data-[open]:translate-x-0"
    };
    let panel = NodeRef::<leptos::html::Div>::new();
    track_overlay_focus(open, panel);
    view! {
        <div
            class=move || {
                if open.get() {
                    "fixed inset-0 z-50 bg-black/30 transition-opacity duration-[450ms] ease-[cubic-bezier(0.32,0.72,0,1)] opacity-100 supports-backdrop-filter:backdrop-blur-sm"
                } else {
                    "fixed inset-0 z-50 bg-black/30 transition-opacity duration-[450ms] ease-[cubic-bezier(0.32,0.72,0,1)] opacity-0 pointer-events-none"
                }
            }
            on:click=move |_| open.set(false)
        ></div>
        <div
            node_ref=panel
            role="dialog"
            aria-modal="true"
            aria-label=move || (!label.is_empty()).then(|| label.clone())
            tabindex="-1"
            data-open=move || open.get().then_some("")
            // Off-screen, not gone — without these the closed panel
            // still takes tab focus and reads to screen readers.
            aria-hidden=move || (!open.get()).then_some("true")
            inert=move || (!open.get()).then_some("")
            class=format!(
                "fixed inset-y-2 z-50 flex w-80 max-w-[85vw] flex-col overflow-hidden rounded-4xl border border-border bg-popover text-sm text-popover-foreground shadow-xl transition-transform duration-[450ms] ease-[cubic-bezier(0.22,1,0.36,1)] focus:outline-none {} {}",
                side_cls, class
            )
            on:keydown=move |ev| {
                if ev.key() == "Escape" {
                    open.set(false);
                }
            }
        >
            {children()}
        </div>
    }
}

clx! { SheetHeader, div, "flex shrink-0 items-center justify-between gap-2 p-4 pb-2" }
clx! { SheetTitle, h3, "font-heading text-base font-medium text-foreground" }
clx! { SheetBody, div, "flex-1 overflow-y-auto px-4 py-3" }

/// Centered empty state — lucide icon tile (the old app's `EmptyMedia`
/// recipe), title, description, actions. `icon` is a lucide name (see
/// `icons::Icon`); empty falls back to `inbox`.
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
pub fn EmptyState(
    #[prop(into)] title: String,
    #[prop(into, optional)] description: String,
    #[prop(into, optional)] icon: String,
    #[prop(into, optional)] class: String,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class=tw_merge::tw_merge!("flex h-full min-h-64 flex-col items-center justify-center gap-4 p-8 text-center", class)>
            <span class="flex size-10 shrink-0 items-center justify-center rounded-xl bg-muted text-foreground">
                <icons::Icon
                    name=if icon.is_empty() { "inbox".to_string() } else { icon }
                    class="size-5"
                />
            </span>
            <h3 class="text-lg font-medium tracking-tight">{title}</h3>
            {if description.is_empty() {
                None
            } else {
                Some(view! { <p class="max-w-sm text-sm text-muted-foreground">{description}</p> })
            }}
            {children.map(|c| c())}
        </div>
    }
}
