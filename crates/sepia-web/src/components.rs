//! shadcn/rust-ui style components on `leptos_ui` (`clx!`, `variants!`,
//! `tw_merge`). All components render deterministic markup — SSR and
//! hydration must agree, so nothing here branches on `Resource` data
//! outside a `Suspend` boundary.

use leptos::prelude::*;
use leptos_ui::{clx, variants, void};

variants! {
    Button {
        base: "inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md font-medium transition-colors focus-visible:outline-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50 cursor-pointer",
        variants: {
            variant: {
                Default: "bg-primary text-primary-foreground shadow hover:bg-primary/90",
                Secondary: "bg-secondary text-secondary-foreground hover:bg-secondary/80",
                Ghost: "hover:bg-accent hover:text-accent-foreground",
                Outline: "border border-input bg-transparent hover:bg-accent hover:text-accent-foreground",
                Destructive: "bg-destructive text-destructive-foreground shadow hover:bg-destructive/90",
                Link: "text-primary underline-offset-4 hover:underline",
            },
            size: {
                Default: "h-9 px-4 py-2 text-sm",
                Sm: "h-8 rounded-md px-3 text-xs",
                Lg: "h-10 rounded-md px-6 text-base",
                Icon: "size-9",
            }
        }
    }
}

#[component]
pub fn Button(
    #[prop(optional)] variant: ButtonVariant,
    #[prop(optional)] size: ButtonSize,
    #[prop(optional)] class: String,
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
    "flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50"
}

/// `<textarea>` / `<select>` need children or multiple attrs — apply
/// these class strings on the native elements instead of components.
pub const TEXTAREA_CLASS: &str = "flex min-h-16 w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm placeholder:text-muted-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50";

pub const SELECT_CLASS: &str = "flex h-9 w-full appearance-none rounded-md border border-input bg-card px-3 py-1 text-sm shadow-sm focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50";

variants! {
    Badge {
        base: "inline-flex w-fit items-center gap-1 rounded-md border px-2 py-0.5 text-[11px] font-semibold tracking-wide uppercase whitespace-nowrap",
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
                Default: "px-2 py-0.5 text-[11px]",
                Sm: "px-1.5 py-px text-[10px]",
            }
        }
    }
}

#[component]
pub fn Badge(
    #[prop(optional)] variant: BadgeVariant,
    #[prop(optional)] class: String,
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
/// closes on outside click via a fixed transparent backdrop.
#[component]
pub fn Dropdown(
    #[prop(into)] label: String,
    #[prop(optional)] class: String,
    children: Children,
) -> impl IntoView {
    view! {
        <details class=tw_merge::tw_merge!("group relative", class)>
            <summary class="list-none cursor-pointer select-none [&::-webkit-details-marker]:hidden">
                <span class=ButtonClass {
                    variant: ButtonVariant::Secondary,
                    size: ButtonSize::Sm,
                }.to_class()>
                    {label}
                    <svg class="size-3 opacity-60 transition-transform group-open:rotate-180" viewBox="0 0 12 12" fill="none">
                        <path d="M3 4.5L6 7.5L9 4.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                    </svg>
                </span>
            </summary>
            // open: sibling backdrop swallows the outside click
            <span class="fixed inset-0 z-40 hidden cursor-default group-open:block" onclick="this.parentElement.removeAttribute('open')"></span>
            <div class="absolute right-0 z-50 mt-1 w-44 rounded-md border bg-popover p-1 shadow-lg">
                {children()}
            </div>
        </details>
    }
}

/// Menu item inside a `Dropdown`.
#[component]
pub fn MenuItem(
    #[prop(into)] label: String,
    #[prop(optional)] destructive: bool,
    #[prop(optional)] on_click: Option<Box<dyn Fn() + Send + Sync + 'static>>,
) -> impl IntoView {
    let cls = if destructive {
        "flex w-full items-center rounded-sm px-2 py-1.5 text-sm text-destructive hover:bg-destructive/10 cursor-pointer"
    } else {
        "flex w-full items-center rounded-sm px-2 py-1.5 text-sm hover:bg-accent cursor-pointer"
    };
    view! {
        <button type="button" class=cls onclick="this.closest('details').removeAttribute('open')"
            on:click=move |ev| {
                if let Some(cb) = &on_click {
                    ev.prevent_default();
                    cb();
                }
            }
        >
            {label}
        </button>
    }
}

/// Confirm dialog — signal-driven modal. The markup is always
/// rendered (SSR/hydrate agree on the DOM); visibility is a class
/// toggle, so no `Show` `Fn` gymnastics.
#[component]
pub fn ConfirmDialog(
    open: RwSignal<bool>,
    #[prop(into)] title: String,
    #[prop(into)] body: String,
    #[prop(into, optional)] confirm_label: String,
    #[prop(optional)] destructive: bool,
    on_confirm: impl Fn() + Send + Sync + 'static,
) -> impl IntoView {
    let on_confirm = std::sync::Arc::new(on_confirm);
    let confirm_label = if confirm_label.is_empty() {
        "Confirm".to_string()
    } else {
        confirm_label
    };
    view! {
        <div
            class=move || {
                if open.get() {
                    "fixed inset-0 z-50 grid place-items-center bg-black/60 p-4"
                } else {
                    "hidden"
                }
            }
            on:click=move |_| open.set(false)
        >
            <div
                class="w-full max-w-md rounded-lg border bg-card p-5 shadow-xl"
                role="dialog"
                aria-modal="true"
                on:click=|ev| ev.stop_propagation()
            >
                <h3 class="text-base font-semibold">{title}</h3>
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

pub mod toast {
    use leptos::prelude::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Level {
        Info,
        Success,
        Error,
    }

    #[derive(Clone, Debug)]
    pub struct Toast {
        pub id: u64,
        pub level: Level,
        pub message: String,
    }

    /// Toast store lives in a context so SSR render order stays
    /// deterministic (empty list on both sides; toasts only ever
    /// appear post-hydration).
    #[derive(Clone, Copy)]
    pub struct ToastStore {
        pub toasts: RwSignal<Vec<Toast>>,
    }

    impl ToastStore {
        pub fn push(&self, level: Level, message: impl Into<String>) {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            self.toasts.update(|t| {
                t.push(Toast {
                    id,
                    level,
                    message: message.into(),
                });
            });
            #[cfg(feature = "hydrate")]
            {
                let store = *self;
                leptos::task::spawn_local(async move {
                    crate::sleep_ms(4_500).await;
                    store.toasts.update(|t| t.retain(|x| x.id != id));
                });
            }
        }

        pub fn success(&self, msg: impl Into<String>) {
            self.push(Level::Success, msg);
        }
        pub fn error(&self, msg: impl Into<String>) {
            self.push(Level::Error, msg);
        }
        pub fn info(&self, msg: impl Into<String>) {
            self.push(Level::Info, msg);
        }
    }

    /// `provide_toaster()` in `App`, then `use_toast()` anywhere.
    pub fn provide_toaster() {
        provide_context(ToastStore {
            toasts: RwSignal::new(Vec::new()),
        });
    }

    pub fn use_toast() -> ToastStore {
        expect_context::<ToastStore>()
    }

    #[component]
    pub fn Toaster() -> impl IntoView {
        let store = expect_context::<ToastStore>();
        view! {
            <div class="pointer-events-none fixed bottom-4 right-4 z-[70] flex w-80 flex-col gap-2">
                <For
                    each=move || store.toasts.get()
                    key=|t| t.id
                    children=move |t| {
                        let (border, icon) = match t.level {
                            Level::Success => ("border-success/40", "text-success"),
                            Level::Error => ("border-destructive/40", "text-destructive"),
                            Level::Info => ("border-info/40", "text-info"),
                        };
                        let dismiss = move || {
                            store.toasts.update(|v| v.retain(|x| x.id != t.id));
                        };
                        view! {
                            <div class=format!("pointer-events-auto flex items-start gap-2 rounded-md border {} bg-popover px-3 py-2.5 shadow-lg", border)>
                                <span class=format!("mt-0.5 {}", icon)>
                                    {match t.level {
                                        Level::Success => "✓",
                                        Level::Error => "✕",
                                        Level::Info => "i",
                                    }}
                                </span>
                                <p class="flex-1 text-sm leading-snug">{t.message}</p>
                                <button
                                    type="button"
                                    class="text-muted-foreground hover:text-foreground text-xs"
                                    on:click=move |_| dismiss()
                                >
                                    "✕"
                                </button>
                            </div>
                        }
                    }
                />
            </div>
        }
    }
}
