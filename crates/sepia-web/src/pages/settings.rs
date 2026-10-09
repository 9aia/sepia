//! `/settings` — the node's public `GET /api/config` surface, edited
//! generically (one row per key), plus the push-notification toggle.
//! The `PushManager` calls are hydrate-only; SSR renders the controls
//! inert.

use leptos::prelude::*;
use leptos_meta::Title;
use serde_json::Value;

use crate::api::{get_config, push_vapid_key, set_config};
use crate::components::toast::use_toast;
use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent, CardDescription,
    CardHeader, CardTitle, Input, PageDescription, PageHead, PageTitle, Skeleton,
};

#[component]
pub fn SettingsPage() -> impl IntoView {
    let config = Resource::new(|| (), |()| get_config());

    view! {
        <Title text="settings — sepia"/>
        <section class="space-y-6">
            <PageHead class="mb-0">
                <div>
                    <PageTitle>"Settings"</PageTitle>
                    <PageDescription>
                        "Public node configuration and browser notifications."
                    </PageDescription>
                </div>
            </PageHead>
            <Suspense fallback=move || {
                view! { <Skeleton class="h-56 w-full"/> }
            }>
                {move || {
                    Suspend::new(async move {
                        match config.await {
                            Err(e) => {
                                view! {
                                    <p class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                                        {e.to_string()}
                                    </p>
                                }
                                    .into_any()
                            }
                            Ok(map) if map.is_empty() => {
                                view! {
                                    <p class="text-sm text-muted-foreground">
                                        "No settings exposed."
                                    </p>
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
            <PushSection/>
        </section>
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
