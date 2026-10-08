//! sepia-push — Web Push notifications over the meta store.
//!
//! VAPID keys generate once and persist under `vapid` in the meta config;
//! subscriptions live under `pushSubscriptions`. Notifications map from
//! `SessionEvent` frames — run finished/errored and permission requests.
//! Expired subscriptions (404/410) prune themselves on send.

use sepia_meta::MetaStore;
use sepia_proto::SessionEvent;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Notification categories a subscription opts into/out of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushPrefs {
    /// Agent run finished / errored.
    #[serde(default = "default_true")]
    pub done: bool,
    /// Agent is waiting for a permission decision.
    #[serde(default = "default_true")]
    pub permission: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushSubscription {
    pub endpoint: String,
    pub keys: SubscriptionKeys,
    #[serde(default)]
    pub prefs: PushPrefs,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubscriptionKeys {
    pub auth: String,
    pub p256dh: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct VapidKeys {
    #[serde(rename = "publicKey")]
    public_key: String,
    #[serde(rename = "privateKey")]
    private_key: String,
}

#[derive(Debug, Error)]
pub enum PushError {
    #[error("vapid: {0}")]
    Vapid(String),
    #[error("send: {0}")]
    Send(String),
}

fn read_subs(meta: &MetaStore) -> Vec<PushSubscription> {
    meta.config()
        .get("pushSubscriptions")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn write_subs(meta: &MetaStore, subs: &[PushSubscription]) {
    meta.set_config("pushSubscriptions", serde_json::json!(subs));
}

/// The push service bound to a meta store — VAPID keys persist so
/// restarts keep existing subscriptions valid.
pub struct PushStore {
    meta: MetaStore,
    keys: VapidKeys,
}

impl PushStore {
    /// Load or mint the VAPID pair for this node.
    ///
    /// # Panics
    /// On VAPID key generation failure (only a broken RNG).
    pub fn open(meta: MetaStore) -> Self {
        let keys = meta
            .config()
            .get("vapid")
            .and_then(|v| serde_json::from_value::<VapidKeys>(v.clone()).ok())
            .unwrap_or_else(|| {
                let generated = generate_vapid();
                meta.set_config("vapid", serde_json::json!(generated));
                generated
            });
        Self { meta, keys }
    }

    /// The public key clients subscribe with.
    pub fn public_key(&self) -> &str {
        &self.keys.public_key
    }

    pub fn list(&self) -> Vec<PushSubscription> {
        read_subs(&self.meta)
    }

    /// Register or refresh a subscription (keyed on endpoint).
    pub fn upsert(&self, sub: PushSubscription) {
        let mut subs = read_subs(&self.meta);
        subs.retain(|s| s.endpoint != sub.endpoint);
        subs.push(sub);
        write_subs(&self.meta, &subs);
    }

    pub fn remove(&self, endpoint: &str) {
        let mut subs = read_subs(&self.meta);
        subs.retain(|s| s.endpoint != endpoint);
        write_subs(&self.meta, &subs);
    }

    /// Send to every subscribed endpoint that opted into `kind`. Returns
    /// the count delivered; expired endpoints (404/410) are pruned.
    ///
    /// # Errors
    /// Never fails on endpoint errors — only on build-time failures
    /// (bad VAPID keys surface at open, not here).
    pub fn send(&self, kind: Kind, title: &str, body: &str, url: &str) -> usize {
        let targets: Vec<PushSubscription> = read_subs(&self.meta)
            .into_iter()
            .filter(|s| match kind {
                Kind::Done => s.prefs.done,
                Kind::Permission => s.prefs.permission,
            })
            .collect();
        let payload = serde_json::json!({
            "title": title,
            "body": body,
            "url": url,
            "tag": format!("sepia-{}", kind.tag()),
        })
        .to_string();
        let mut delivered = 0usize;
        for sub in &targets {
            match send_one(&self.keys, sub, &payload) {
                Ok(()) => delivered += 1,
                Err(PushError::Send(e)) if e.contains("404") || e.contains("410") => {
                    self.remove(&sub.endpoint);
                }
                Err(_) => {}
            }
        }
        delivered
    }
}

/// The two push categories.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Done,
    Permission,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Permission => "permission",
        }
    }
}

fn generate_vapid() -> VapidKeys {
    use base64::Engine;
    use p256::ecdsa::SigningKey;
    let signing = SigningKey::random(&mut rand::rngs::OsRng);
    let verifying = signing.verifying_key();
    let public_key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(verifying.to_encoded_point(false).as_bytes());
    // VAPID needs the raw 32-byte private scalar, base64url'd — pkcs8
    // DER would wrap it in a structure web-push can't read.
    let private_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signing.to_bytes());
    VapidKeys {
        public_key,
        private_key,
    }
}

fn send_one(keys: &VapidKeys, sub: &PushSubscription, payload: &str) -> Result<(), PushError> {
    use web_push::{
        ContentEncoding, IsahcWebPushClient, SubscriptionInfo, VapidSignatureBuilder,
        WebPushClient, WebPushMessageBuilder,
    };
    let info = SubscriptionInfo::new(
        sub.endpoint.clone(),
        sub.keys.p256dh.clone(),
        sub.keys.auth.clone(),
    );
    let mut sig_builder = VapidSignatureBuilder::from_base64(&keys.private_key, &info)
        .map_err(|e| PushError::Vapid(format!("{e:?}")))?;
    sig_builder.add_claim("sub", "mailto:sepia@localhost");
    let sig = sig_builder
        .build()
        .map_err(|e| PushError::Vapid(format!("{e:?}")))?;
    let mut builder = WebPushMessageBuilder::new(&info);
    builder.set_payload(ContentEncoding::Aes128Gcm, payload.as_bytes());
    builder.set_vapid_signature(sig);
    builder.set_ttl(300);
    let message = builder
        .build()
        .map_err(|e| PushError::Send(format!("{e:?}")))?;
    // Blocking client — push send is a rare, short call.
    let client = IsahcWebPushClient::new().map_err(|e| PushError::Send(format!("{e:?}")))?;
    futures::executor::block_on(client.send(message)).map_err(|e| PushError::Send(format!("{e:?}")))
}

/// Session-event → notification mapping — run completion and permission
/// requests, matching the TS `notifyForEvents`.
pub fn notify_for_events(
    push: &PushStore,
    session_id: &str,
    agent: Option<&str>,
    session_title: &str,
    events: &[SessionEvent],
) {
    let key = agent.map_or_else(|| session_id.to_string(), |a| format!("{a}:{session_id}"));
    let url = format!("/?session={}", urlencoding(&key));
    if events
        .iter()
        .any(|e| matches!(e, SessionEvent::RunFinished { .. }))
    {
        push.send(
            Kind::Done,
            "Session finished",
            &format!("{session_title} finished its run."),
            &url,
        );
    }
    let wants_permission = events.iter().any(
        |e| matches!(e, SessionEvent::Custom { name, .. } if name == "acp:permission_request"),
    );
    if wants_permission {
        push.send(
            Kind::Permission,
            "Approval needed",
            &format!("{session_title} is waiting for you."),
            &url,
        );
    }
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.~".contains(c) {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}
