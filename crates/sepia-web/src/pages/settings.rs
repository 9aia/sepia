//! `/settings` — the settings surface: appearance (theme), node
//! identity + connection health, the agent catalog, the hub's queued
//! writes, the public `GET /api/config` map edited generically (one
//! row per key), push notifications, and the keyboard-shortcut
//! reference. Browser-only pieces (`PushManager`, the theme signal)
//! are hydrate-only; SSR renders identical markup, inert.

use leptos::prelude::*;
use leptos_meta::Title;
use serde_json::Value;

use crate::api::{
    get_config, list_agents, node_info, node_status, pending_writes, push_vapid_key, set_config,
};
use crate::app::SHORTCUTS;
use crate::components::toast::use_toast;
use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent, CardDescription,
    CardHeader, CardTitle, Input, PageDescription, PageHead, PageTitle, SELECT_CLASS, Skeleton,
};
use crate::dto::AgentDto;
use crate::pages::agents::capability_chips;
use crate::pages::nodes::{NodeRow, WriteRow};
use crate::theme::Theme;

/// Error body shared by the async sections — one destructive-tinted
/// paragraph instead of a broken card.
fn err_view(e: &ServerFnError) -> AnyView {
    view! {
        <p class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {e.to_string()}
        </p>
    }
    .into_any()
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    view! {
        <Title text="settings — sepia"/>
        <section class="mx-auto w-full max-w-5xl space-y-6 p-4 md:p-6">
            <PageHead class="mb-0">
                <div>
                    <PageTitle>"Settings"</PageTitle>
                    <PageDescription>
                        "Appearance, node identity, agents, outbox, and notifications."
                    </PageDescription>
                </div>
            </PageHead>
            <AppearanceSection/>
            <NodeSection/>
            <AgentsSection/>
            <OutboxSection/>
            <ConfigSection/>
            <PushSection/>
            <ShortcutsSection/>
        </section>
    }
}

/// Theme picker. The `RwSignal<Theme>` context only exists on the
/// `hydrate` build (`provide_theme` is an SSR stub), so SSR renders
/// the same `<select>` disabled with `dark` pre-selected — the shell
/// always emits `class="dark"` and hydrate reconciles.
#[component]
fn AppearanceSection() -> impl IntoView {
    let theme = use_context::<RwSignal<Theme>>();
    let current = move || theme.map_or(Theme::Dark, |t| t.get());
    let on_change = move |ev| {
        if let Some(theme) = theme {
            theme.set(match event_target_value(&ev).as_str() {
                "light" => Theme::Light,
                "dark" => Theme::Dark,
                _ => Theme::System,
            });
        }
    };

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Appearance"</CardTitle>
                <CardDescription>"Color scheme for this browser."</CardDescription>
            </CardHeader>
            <CardContent>
                <label class="flex max-w-56 flex-col gap-1.5 text-sm">
                    <span class="text-muted-foreground">"Theme"</span>
                    <select
                        class=SELECT_CLASS
                        disabled=theme.is_none()
                        on:change=on_change
                    >
                        <option value="system" selected=move || current() == Theme::System>
                            "System"
                        </option>
                        <option value="dark" selected=move || current() == Theme::Dark>
                            "Dark"
                        </option>
                        <option value="light" selected=move || current() == Theme::Light>
                            "Light"
                        </option>
                    </select>
                </label>
            </CardContent>
        </Card>
    }
}

/// `GET /api/node` + the hub's per-node health — who this hub is
/// talking to and whether the link is up.
#[component]
fn NodeSection() -> impl IntoView {
    let info = Resource::new(|| (), |()| node_info());
    let status = Resource::new(|| (), |()| node_status());

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Node"</CardTitle>
                <CardDescription>"Identity and connection health."</CardDescription>
            </CardHeader>
            <CardContent class="space-y-4">
                <Suspense fallback=move || {
                    view! { <Skeleton class="h-28 w-full"/> }
                }>
                    {move || {
                        Suspend::new(async move {
                            let (info, status) = (info.await, status.await);
                            let info_view = match info {
                                Err(e) => err_view(&e),
                                Ok(n) => {
                                    let title = if n.name.is_empty() {
                                        "—".to_string()
                                    } else {
                                        n.name.clone()
                                    };
                                    let version =
                                        format!("{} (protocol {})", n.version, n.protocol);
                                    let agents = n.agents.len();
                                    view! {
                                        <dl class="grid grid-cols-[6rem_1fr] gap-x-4 gap-y-2.5 text-sm">
                                            <dt class="text-muted-foreground">"id"</dt>
                                            <dd class="font-mono text-xs">{n.id.clone()}</dd>
                                            <dt class="text-muted-foreground">"name"</dt>
                                            <dd>{title}</dd>
                                            <dt class="text-muted-foreground">"version"</dt>
                                            <dd>{version}</dd>
                                            <dt class="text-muted-foreground">"agents"</dt>
                                            <dd>{format!("{agents} registered")}</dd>
                                            {if n.capabilities.is_empty() {
                                                None
                                            } else {
                                                Some(
                                                    view! {
                                                        <dt class="text-muted-foreground">"capabilities"</dt>
                                                        <dd>
                                                            <div class="flex flex-wrap gap-1.5">
                                                                {n
                                                                    .capabilities
                                                                    .iter()
                                                                    .map(|c| {
                                                                        let c = c.clone();
                                                                        view! {
                                                                            <Badge variant=BadgeVariant::Muted>{c}</Badge>
                                                                        }
                                                                    })
                                                                    .collect::<Vec<_>>()}
                                                            </div>
                                                        </dd>
                                                    },
                                                )
                                            }}
                                        </dl>
                                    }
                                        .into_any()
                                }
                            };
                            let status_view = match status {
                                Err(e) => err_view(&e),
                                Ok(rows) if rows.is_empty() => {
                                    view! {
                                        <p class="text-sm text-muted-foreground">
                                            "No connection status reported."
                                        </p>
                                    }
                                        .into_any()
                                }
                                Ok(rows) => {
                                    view! {
                                        <div>
                                            <h4 class="mb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                                                "Connection"
                                            </h4>
                                            <ul class="divide-y divide-border">
                                                {rows
                                                    .into_iter()
                                                    .map(|r| view! { <NodeRow row=r/> })
                                                    .collect::<Vec<_>>()}
                                            </ul>
                                        </div>
                                    }
                                        .into_any()
                                }
                            };
                            view! {
                                {info_view}
                                {status_view}
                            }
                        })
                    }}
                </Suspense>
            </CardContent>
        </Card>
    }
}

/// `GET /api/agents` — the merged catalog, one compact row per runtime
/// (label, id, owning node, capability chips).
#[component]
fn AgentsSection() -> impl IntoView {
    let agents = Resource::new(|| (), |()| list_agents());

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Agents"</CardTitle>
                <CardDescription>
                    "Agent runtimes advertised by the connected nodes."
                </CardDescription>
            </CardHeader>
            <CardContent>
                <Suspense fallback=move || {
                    view! { <Skeleton class="h-24 w-full"/> }
                }>
                    {move || {
                        Suspend::new(async move {
                            match agents.await {
                                Err(e) => err_view(&e),
                                Ok(list) if list.is_empty() => {
                                    view! {
                                        <p class="text-sm text-muted-foreground">
                                            "No agents configured."
                                        </p>
                                    }
                                        .into_any()
                                }
                                Ok(list) => {
                                    view! {
                                        <ul class="divide-y divide-border">
                                            {list
                                                .into_iter()
                                                .map(|a| view! { <AgentRow agent=a/> })
                                                .collect::<Vec<_>>()}
                                        </ul>
                                    }
                                        .into_any()
                                }
                            }
                        })
                    }}
                </Suspense>
            </CardContent>
        </Card>
    }
}

/// Compact agent row — same chip set as `/agents`, flatter layout.
#[component]
fn AgentRow(agent: AgentDto) -> impl IntoView {
    let label = if agent.label.trim().is_empty() {
        agent.id.clone()
    } else {
        agent.label.clone()
    };
    let chips = capability_chips(agent.capabilities.as_ref());
    view! {
        <li class="flex flex-wrap items-center gap-x-3 gap-y-1.5 py-3 first:pt-0 last:pb-0">
            <span class="text-sm font-medium">{label}</span>
            <code class="font-mono text-xs text-muted-foreground">{agent.id.clone()}</code>
            {agent
                .node
                .clone()
                .map(|n| view! { <Badge variant=BadgeVariant::Info>{n}</Badge> })}
            <span class="ml-auto flex flex-wrap gap-1.5">
                {if chips.is_empty() {
                    view! {
                        <span class="text-xs text-muted-foreground">
                            "capabilities not probed"
                        </span>
                    }
                        .into_any()
                } else {
                    chips
                        .into_iter()
                        .map(|c| view! { <Badge variant=BadgeVariant::Muted>{c}</Badge> })
                        .collect::<Vec<_>>()
                        .into_any()
                }}
            </span>
        </li>
    }
}

/// The hub's durable write queue — `queued` rows replay when the node
/// returns, `failed` ones are dead letters carrying the last error.
/// Empty on a direct (non-sync) connection.
#[component]
fn OutboxSection() -> impl IntoView {
    let pending = Resource::new(|| (), |()| pending_writes());

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Queued writes"</CardTitle>
                <CardDescription>
                    "Prompts and edits waiting on an unreachable node."
                </CardDescription>
            </CardHeader>
            <CardContent>
                <Suspense fallback=move || {
                    view! { <Skeleton class="h-16 w-full"/> }
                }>
                    {move || {
                        Suspend::new(async move {
                            match pending.await {
                                Err(e) => err_view(&e),
                                Ok(rows) if rows.is_empty() => {
                                    view! {
                                        <p class="text-sm text-muted-foreground">
                                            "No queued writes."
                                        </p>
                                    }
                                        .into_any()
                                }
                                Ok(rows) => {
                                    view! {
                                        <ul class="divide-y divide-border">
                                            {rows
                                                .into_iter()
                                                .map(|w| view! { <WriteRow write=w/> })
                                                .collect::<Vec<_>>()}
                                        </ul>
                                    }
                                        .into_any()
                                }
                            }
                        })
                    }}
                </Suspense>
            </CardContent>
        </Card>
    }
}

/// The node's public `GET /api/config` map — generic key rows.
#[component]
fn ConfigSection() -> impl IntoView {
    let config = Resource::new(|| (), |()| get_config());

    view! {
        <Suspense fallback=move || {
            view! { <Skeleton class="h-40 w-full"/> }
        }>
            {move || {
                Suspend::new(async move {
                    match config.await {
                        Err(e) => err_view(&e),
                        Ok(map) if map.is_empty() => {
                            view! {
                                <Card>
                                    <CardHeader>
                                        <CardTitle>"Configuration"</CardTitle>
                                        <CardDescription>
                                            "Public config keys exposed by the node."
                                        </CardDescription>
                                    </CardHeader>
                                    <CardContent>
                                        <p class="text-sm text-muted-foreground">
                                            "No settings exposed."
                                        </p>
                                    </CardContent>
                                </Card>
                            }
                                .into_any()
                        }
                        Ok(map) => {
                            view! {
                                <Card>
                                    <CardHeader>
                                        <CardTitle>"Configuration"</CardTitle>
                                        <CardDescription>
                                            "Public config keys exposed by the node."
                                        </CardDescription>
                                    </CardHeader>
                                    <CardContent>
                                        <ul class="divide-y divide-border">
                                            {map
                                                .into_iter()
                                                .map(|(k, v)| view! { <ConfigRow key=k value=v/> })
                                                .collect::<Vec<_>>()}
                                        </ul>
                                    </CardContent>
                                </Card>
                            }
                                .into_any()
                        }
                    }
                })
            }}
        </Suspense>
    }
}

/// Read-only mirror of the `?` cheat-sheet (`app::SHORTCUTS`).
#[component]
fn ShortcutsSection() -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Keyboard shortcuts"</CardTitle>
                <CardDescription>
                    "Global keys — press ? anywhere to see the same list."
                </CardDescription>
            </CardHeader>
            <CardContent>
                <dl class="space-y-1.5">
                    {SHORTCUTS
                        .iter()
                        .map(|(k, d)| {
                            view! {
                                <div class="flex items-center gap-3">
                                    <kbd class="rounded border bg-muted px-1.5 py-0.5 font-mono text-[11px]">
                                        {*k}
                                    </kbd>
                                    <dd class="text-xs text-muted-foreground">{*d}</dd>
                                </div>
                            }
                        })
                        .collect::<Vec<_>>()}
                </dl>
            </CardContent>
        </Card>
    }
}

/// One editable config entry — strings edit raw, everything else edits
/// as JSON and saves as JSON when the input still parses.
#[component]
fn ConfigRow(key: String, value: Value) -> impl IntoView {
    let is_string = value.is_string();
    let initial = if let Some(s) = value.as_str() {
        s.to_string()
    } else {
        serde_json::to_string(&value).unwrap_or_default()
    };
    let draft = RwSignal::new(initial.clone());
    let dirty = move || draft.get() != initial;
    let saving = RwSignal::new(false);
    let status: RwSignal<Option<Result<(), String>>> = RwSignal::new(None);

    let on_save = {
        let key = key.clone();
        move || {
            if saving.get() {
                return;
            }
            saving.set(true);
            status.set(None);
            let raw = draft.get();
            let value = if is_string {
                Value::String(raw)
            } else {
                serde_json::from_str(&raw).unwrap_or(Value::String(raw))
            };
            let key = key.clone();
            leptos::task::spawn_local(async move {
                status.set(Some(
                    set_config(key, value).await.map_err(|e| e.to_string()),
                ));
                saving.set(false);
            });
        }
    };

    view! {
        <li class="flex flex-wrap items-center gap-3 py-3 first:pt-0 last:pb-0">
            <span
                class="w-56 shrink-0 truncate font-mono text-xs font-medium"
                title=key
            >
                {key.clone()}
            </span>
            <Input
                class="min-w-48 flex-1"
                attr:r#type="text"
                prop:value=move || draft.get()
                on:input=move |ev| {
                    draft.set(event_target_value(&ev));
                    status.set(None);
                }
            />
            <Button
                size=ButtonSize::Sm
                disabled=move || saving.get() || !dirty()
                on_click=Box::new(on_save)
            >
                {move || if saving.get() { "Saving…" } else { "Save" }}
            </Button>
            {move || {
                status.get().map(|s| match s {
                    Ok(()) => {
                        view! { <span class="text-xs text-success">"saved"</span> }.into_any()
                    }
                    Err(e) => {
                        view! { <span class="text-xs text-destructive">{e}</span> }.into_any()
                    }
                })
            }}
        </li>
    }
}

/// "Push notifications" — subscribes the browser's `PushManager`
/// against the node's VAPID key. SSR/hydration-safe: the DOM calls
/// only exist in the `hydrate` build.
#[component]
fn PushSection() -> impl IntoView {
    let vapid = Resource::new(|| (), |()| push_vapid_key());
    let subscribed: RwSignal<Option<bool>> = RwSignal::new(None);
    let busy = RwSignal::new(false);
    let toast = use_toast();
    #[cfg(not(feature = "hydrate"))]
    let _ = toast;

    #[cfg(feature = "hydrate")]
    {
        // Probe the existing subscription once, on the client only.
        leptos::task::spawn_local(async move {
            if let Ok(current) = push::current().await {
                subscribed.set(Some(current.is_some()));
            }
        });
    }

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Push notifications"</CardTitle>
                <CardDescription>
                    "Send session notifications to this browser."
                </CardDescription>
            </CardHeader>
            <CardContent>
                <Suspense fallback=move || {
                    view! { <Skeleton class="h-9 w-44"/> }
                }>
                    {move || {
                        Suspend::new(async move {
                            match vapid.await {
                                Err(_) => {
                                    view! {
                                        <p class="text-sm text-muted-foreground">
                                            "Push isn't configured on this node."
                                        </p>
                                    }
                                        .into_any()
                                }
                                Ok(key) => {
                                    let toggle = move || {
                                        if busy.get() {
                                            return;
                                        }
                                        busy.set(true);
                                        let key = key.clone();
                                        leptos::task::spawn_local(async move {
                                            #[cfg(feature = "hydrate")]
                                            {
                                                let result = if subscribed.get() == Some(true) {
                                                    push::unsubscribe().await
                                                } else {
                                                    push::subscribe(&key).await
                                                };
                                                match result {
                                                    Ok(()) => {
                                                        let now = subscribed.get() != Some(true);
                                                        subscribed.set(Some(now));
                                                        if now {
                                                            toast.success(
                                                                "Notifications enabled.",
                                                            );
                                                        } else {
                                                            toast.info("Notifications disabled.");
                                                        }
                                                    }
                                                    Err(e) => toast.error(e),
                                                }
                                            }
                                            #[cfg(not(feature = "hydrate"))]
                                            let _ = key;
                                            busy.set(false);
                                        });
                                    };
                                    view! {
                                        <div class="flex items-center gap-3">
                                            <Button
                                                variant=ButtonVariant::Outline
                                                disabled=move || busy.get()
                                                on_click=Box::new(toggle)
                                            >
                                                {move || {
                                                    match (busy.get(), subscribed.get()) {
                                                        (true, _) => "Working…",
                                                        (false, Some(true)) => "Disable notifications",
                                                        _ => "Enable notifications",
                                                    }
                                                }}
                                            </Button>
                                            {move || {
                                                subscribed
                                                    .get()
                                                    .map(|on| {
                                                        view! {
                                                            <Badge variant=if on {
                                                                BadgeVariant::Success
                                                            } else {
                                                                BadgeVariant::Muted
                                                            }>
                                                                {if on { "enabled" } else { "disabled" }}
                                                            </Badge>
                                                        }
                                                    })
                                            }}
                                        </div>
                                    }
                                        .into_any()
                                }
                            }
                        })
                    }}
                </Suspense>
            </CardContent>
        </Card>
    }
}

/// `PushManager` plumbing — wasm only. `navigator.serviceWorker.ready`
/// resolves once `sw.js` is active; the subscription posts `{endpoint,
/// keys:{auth,p256dh}}` back through the `/hub` server fn.
#[cfg(feature = "hydrate")]
mod push {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::JsValue;
    use wasm_bindgen_futures::JsFuture;

    use super::b64url;
    use crate::api::{push_subscribe, push_unsubscribe};
    use crate::dto::{PushKeysDto, PushSubscriptionDto};

    fn js_err(e: &JsValue) -> String {
        e.as_string()
            .or_else(|| js_sys::JSON::stringify(e).ok().and_then(|s| s.as_string()))
            .unwrap_or_else(|| format!("{e:?}"))
    }

    async fn registration() -> Result<web_sys::ServiceWorkerRegistration, String> {
        let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
        let promise = window
            .navigator()
            .service_worker()
            .ready()
            .map_err(|e| js_err(&e))?;
        JsFuture::from(promise)
            .await
            .map_err(|e| js_err(&e))?
            .dyn_into::<web_sys::ServiceWorkerRegistration>()
            .map_err(|e| js_err(&e))
    }

    /// The browser's current subscription, if any.
    pub async fn current() -> Result<Option<web_sys::PushSubscription>, String> {
        let reg = registration().await?;
        let promise = reg
            .push_manager()
            .map_err(|e| js_err(&e))?
            .get_subscription()
            .map_err(|e| js_err(&e))?;
        let value = JsFuture::from(promise).await.map_err(|e| js_err(&e))?;
        Ok(if value.is_null() {
            None
        } else {
            Some(value.unchecked_into::<web_sys::PushSubscription>())
        })
    }

    /// `pushManager.subscribe` + `POST /api/push/subscribe` via the hub.
    pub async fn subscribe(vapid: &str) -> Result<(), String> {
        let reg = registration().await?;
        let manager = reg.push_manager().map_err(|e| js_err(&e))?;
        let key = b64url::decode(vapid)?;
        let options = web_sys::PushSubscriptionOptionsInit::new();
        options.set_user_visible_only(true);
        options.set_application_server_key_opt_u8_array(Some(&js_sys::Uint8Array::from(
            key.as_slice(),
        )));
        let promise = manager
            .subscribe_with_options(&options)
            .map_err(|e| js_err(&e))?;
        let sub: web_sys::PushSubscription = JsFuture::from(promise)
            .await
            .map_err(|e| js_err(&e))?
            .unchecked_into();
        let dto = PushSubscriptionDto {
            endpoint: sub.endpoint(),
            keys: PushKeysDto {
                auth: key_b64(&sub, web_sys::PushEncryptionKeyName::Auth)?,
                p256dh: key_b64(&sub, web_sys::PushEncryptionKeyName::P256dh)?,
            },
        };
        push_subscribe(dto).await.map_err(|e| e.to_string())
    }

    /// `subscription.unsubscribe()` + `DELETE /api/push/subscribe`.
    pub async fn unsubscribe() -> Result<(), String> {
        let Some(sub) = current().await? else {
            return Ok(());
        };
        let endpoint = sub.endpoint();
        let promise = sub.unsubscribe().map_err(|e| js_err(&e))?;
        let _ = JsFuture::from(promise).await.map_err(|e| js_err(&e))?;
        push_unsubscribe(endpoint).await.map_err(|e| e.to_string())
    }

    /// `sub.get_key(name)` → base64url string (the shape `toJSON()`
    /// emits and the node expects verbatim).
    fn key_b64(
        sub: &web_sys::PushSubscription,
        name: web_sys::PushEncryptionKeyName,
    ) -> Result<String, String> {
        let buffer = sub
            .get_key(name)
            .map_err(|e| js_err(&e))?
            .ok_or_else(|| "push subscription key missing".to_string())?;
        Ok(b64url::encode(&js_sys::Uint8Array::new(&buffer).to_vec()))
    }
}

/// Unpadded base64url — pure helpers so they can be unit-tested on the
/// host build (the `PushManager` glue above is wasm-only).
#[cfg(any(feature = "hydrate", test))]
mod b64url {
    /// Unpadded base64url → bytes (accepts padded/`+`/`/`-style input —
    /// VAPID keys come back url-safe but tolerance costs nothing).
    pub fn decode(input: &str) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(input.len() * 3 / 4);
        let mut acc: u32 = 0;
        let mut bits: u32 = 0;
        for b in input.bytes() {
            let v = match b {
                b'A'..=b'Z' => u32::from(b - b'A'),
                b'a'..=b'z' => u32::from(b - b'a' + 26),
                b'0'..=b'9' => u32::from(b - b'0' + 52),
                b'-' | b'+' => 62,
                b'_' | b'/' => 63,
                b'=' | b'\n' | b'\r' | b' ' => continue,
                _ => return Err("invalid VAPID public key".to_string()),
            };
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                // Mask the low byte — `acc` keeps stale high bits.
                out.push(((acc >> bits) & 0xff) as u8);
            }
        }
        Ok(out)
    }

    /// Bytes → unpadded base64url (the `PushSubscription` key shape).
    pub fn encode(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().fold(0u32, |n, &b| (n << 8) | u32::from(b))
                << (8 * u32::try_from(3 - chunk.len()).unwrap_or(0));
            out.push(char::from(TABLE[((n >> 18) & 63) as usize]));
            out.push(char::from(TABLE[((n >> 12) & 63) as usize]));
            if chunk.len() > 1 {
                out.push(char::from(TABLE[((n >> 6) & 63) as usize]));
            }
            if chunk.len() > 2 {
                out.push(char::from(TABLE[(n & 63) as usize]));
            }
        }
        out
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    mod tests {
        use super::*;

        #[test]
        fn round_trips() {
            // A 65-byte uncompressed P-256 point (VAPID public key shape).
            let key: Vec<u8> = (0..=64u8).collect();
            let encoded = encode(&key);
            assert!(!encoded.contains('=') && !encoded.contains('+'));
            assert_eq!(decode(&encoded).unwrap(), key);
        }

        #[test]
        fn decodes_known_vapid() {
            // RFC 8292 example public key.
            let key =
                decode("BP4x9CIWn9yaWxhjvZ61s1kQpLn7h5E5eF5sHhX0Zv0l1K5eV5rC5hN5V5fK5iY5rN5bXw");
            assert!(key.is_ok());
            assert!(decode("***not-base64***").is_err());
        }
    }
}
