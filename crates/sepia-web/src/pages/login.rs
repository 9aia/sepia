//! `/login` — the hub-token gate (`SEPIA_HUB_TOKEN`). Pre-auth by
//! definition: the page renders inside the app shell but covers it
//! with a fixed full-viewport card so the gated nav never leaks.
//!
//! The form is a plain `POST` to the hub (`auth::login`) — no server
//! fn, so it works identically pre-hydration: a good token plants the
//! httpOnly `sepia_hub` cookie and 303s to `next` (or `/`); a bad one
//! 303s back here with `?error=1`. `?token=<value>` in the URL is the
//! other credential path — the gate validates it before the page
//! renders, so if the param survives to render, it was wrong and the
//! error shows.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;

use crate::components::{Button, Card, CardContent, CardDescription, CardHeader, CardTitle, Input};

#[component]
pub fn LoginPage() -> impl IntoView {
    let query = use_query_map();
    // A failed POST lands back on `?error=1`; a `?token=` that reached
    // the page was rejected by the gate. Both mean "wrong token".
    let failed = move || {
        let q = query.read();
        q.get("error").is_some() || q.get("token").is_some()
    };
    // `?next=/…` — where the gate sent us from; preserved through the
    // POST via a hidden field.
    let next = move || query.read().get("next").unwrap_or_default();

    view! {
        <Title text="sign in — sepia"/>
        <div
            data-name="LoginPage"
            class="fixed inset-0 z-[90] grid place-items-center bg-background p-4"
        >
            <Card class="w-full max-w-sm">
                <CardHeader>
                    <span class="text-lg font-bold tracking-tight text-primary">"sepia"</span>
                    <CardTitle>"Sign in"</CardTitle>
                    <CardDescription>
                        "This hub is locked — enter the access token (SEPIA_HUB_TOKEN)."
                    </CardDescription>
                </CardHeader>
                <CardContent>
                    <form method="post" action="/login" class="flex flex-col gap-3">
                        {move || {
                            let n = next();
                            (!n.is_empty()).then(|| {
                                view! { <input type="hidden" name="next" value=n/> }
                            })
                        }}
                        <label class="flex flex-col gap-1.5 text-sm">
                            <span class="text-muted-foreground">"Access token"</span>
                            <Input
                                {..}
                                attr:r#type="password"
                                attr:name="token"
                                attr:placeholder="hub token"
                                attr:autocomplete="current-password"
                                attr:required=true
                                attr:autofocus=true
                            />
                        </label>
                        {move || {
                            failed().then(|| {
                                view! {
                                    <p
                                        data-name="LoginError"
                                        role="alert"
                                        class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                                    >
                                        "Invalid token — check it and try again."
                                    </p>
                                }
                            })
                        }}
                        <Button button_type="submit">"Continue"</Button>
                    </form>
                    <p class="mt-4 text-xs text-muted-foreground">
                        "One-time link? "
                        <code class="font-mono">"/?token=…"</code>
                        " plants the same cookie."
                    </p>
                </CardContent>
            </Card>
        </div>
    }
}
