//! Per-session permission policy — how a live session settles
//! `session/request_permission` requests. `ask` is the historical
//! behavior (broadcast, wait for `sessions.permission`); unattended
//! runs need a non-`ask` policy or the agent deadlocks mid-turn.

use sepia_acp::{PermissionOption, PermissionRequest};
use serde::{Deserialize, Serialize};

/// What the plane does with one permission request under the session's
/// [`PermissionPolicy`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionAction {
    /// Broadcast only — a subscriber settles it via `sessions.permission`.
    Ask,
    /// Auto-select an allow option.
    Allow,
    /// Auto-select a reject option, else the cancelled outcome.
    Deny,
}

/// One rule inside [`PermissionPolicy::Rules`] — first match wins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRule {
    /// ACP tool-call `kind` to match (`execute`, `edit`, `read`, …);
    /// absent matches every request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `true` auto-allows, `false` auto-denies.
    pub allow: bool,
}

/// How a live session answers `session/request_permission` — set at
/// create/attach or switched live via `sessions.set_permission_policy`.
/// The request event still broadcasts under every policy, so
/// subscribers observe asks the policy auto-settled.
///
/// Wire form (`sessions.create`/`sessions.attach` `permissions` param,
/// `sessions.set_permission_policy`): `"ask"`, `"allow_all"`,
/// `"deny_all"`, or `{"rules": [{"kind": "execute", "allow": false}, …]}`
/// — unmatched requests under `rules` fall back to `ask`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    /// Broadcast to subscribers — nothing is auto-answered.
    #[default]
    Ask,
    /// Auto-approve every request.
    AllowAll,
    /// Auto-deny every request.
    DenyAll,
    /// First matching [`PermissionRule`] decides; unmatched requests
    /// fall back to `ask`.
    Rules(Vec<PermissionRule>),
}

impl PermissionPolicy {
    /// The action for one request — `Rules` matches on the tool-call
    /// kind (absent `kind` is a wildcard), first rule wins, no match
    /// asks.
    #[must_use]
    pub fn evaluate(&self, request: &PermissionRequest) -> PermissionAction {
        match self {
            Self::Ask => PermissionAction::Ask,
            Self::AllowAll => PermissionAction::Allow,
            Self::DenyAll => PermissionAction::Deny,
            Self::Rules(rules) => rules
                .iter()
                .find(|rule| {
                    rule.kind
                        .as_deref()
                        .is_none_or(|kind| request.tool_kind.as_deref() == Some(kind))
                })
                .map_or(PermissionAction::Ask, |rule| {
                    if rule.allow {
                        PermissionAction::Allow
                    } else {
                        PermissionAction::Deny
                    }
                }),
        }
    }
}

/// Option kinds in preference order — the `_once` grants first:
/// per-request answers keep the grant least-privilege; the agent
/// re-asks (and the policy re-answers) rather than the allow silently
/// widening agent-side.
const ALLOW_KINDS: [&str; 2] = ["allow_once", "allow_always"];
const DENY_KINDS: [&str; 2] = ["reject_once", "reject_always"];

/// The `optionId` that settles `request` for `action` — `None` sends
/// the cancelled outcome. Agents advertising non-standard kinds fall
/// back to an `allow*`/`reject*` prefix match; nothing matched means
/// cancel, never a guessed option.
#[must_use]
pub fn settle_option(request: &PermissionRequest, action: PermissionAction) -> Option<String> {
    let (kinds, prefix) = match action {
        PermissionAction::Allow => (ALLOW_KINDS.as_slice(), "allow"),
        PermissionAction::Deny => (DENY_KINDS.as_slice(), "reject"),
        PermissionAction::Ask => return None,
    };
    for kind in kinds {
        if let Some(option) = find_option(request, |o| o.kind == *kind) {
            return Some(option.option_id.clone());
        }
    }
    find_option(request, |o| o.kind.starts_with(prefix)).map(|o| o.option_id.clone())
}

fn find_option<'a>(
    request: &'a PermissionRequest,
    pred: impl Fn(&'a PermissionOption) -> bool,
) -> Option<&'a PermissionOption> {
    request
        .options
        .iter()
        .find(|o| !o.option_id.is_empty() && pred(o))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn request(kind: Option<&str>, options: &[(&str, &str)]) -> PermissionRequest {
        PermissionRequest {
            request_id: "r1".into(),
            session_id: "s1".into(),
            tool_call_id: Some("tc".into()),
            tool_kind: kind.map(str::to_string),
            title: "Run it".into(),
            options: options
                .iter()
                .map(|(id, kind)| PermissionOption {
                    option_id: (*id).into(),
                    name: (*id).into(),
                    kind: (*kind).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn blanket_policies_evaluate() {
        let req = request(Some("execute"), &[]);
        assert_eq!(PermissionPolicy::Ask.evaluate(&req), PermissionAction::Ask);
        assert_eq!(
            PermissionPolicy::AllowAll.evaluate(&req),
            PermissionAction::Allow
        );
        assert_eq!(
            PermissionPolicy::DenyAll.evaluate(&req),
            PermissionAction::Deny
        );
    }

    #[test]
    fn rules_match_kind_first_wins() {
        let policy = PermissionPolicy::Rules(vec![
            PermissionRule {
                kind: Some("execute".into()),
                allow: false,
            },
            PermissionRule {
                kind: None,
                allow: true,
            },
        ]);
        assert_eq!(
            policy.evaluate(&request(Some("execute"), &[])),
            PermissionAction::Deny
        );
        assert_eq!(
            policy.evaluate(&request(Some("edit"), &[])),
            PermissionAction::Allow
        );
        // No kind reported → only the wildcard rule matches.
        assert_eq!(
            policy.evaluate(&request(None, &[])),
            PermissionAction::Allow
        );
    }

    #[test]
    fn rules_without_match_ask() {
        let policy = PermissionPolicy::Rules(vec![PermissionRule {
            kind: Some("execute".into()),
            allow: true,
        }]);
        assert_eq!(
            policy.evaluate(&request(Some("edit"), &[])),
            PermissionAction::Ask
        );
    }

    #[test]
    fn settle_prefers_once_then_always_then_prefix() {
        let req = request(
            Some("execute"),
            &[
                ("always", "allow_always"),
                ("once", "allow_once"),
                ("reject", "reject_once"),
            ],
        );
        assert_eq!(
            settle_option(&req, PermissionAction::Allow).as_deref(),
            Some("once")
        );
        assert_eq!(
            settle_option(&req, PermissionAction::Deny).as_deref(),
            Some("reject")
        );
        let custom = request(Some("execute"), &[("ok", "permit"), ("no", "deny")]);
        assert_eq!(settle_option(&custom, PermissionAction::Allow), None);
        let prefixed = request(Some("execute"), &[("ok", "allow_custom")]);
        assert_eq!(
            settle_option(&prefixed, PermissionAction::Allow).as_deref(),
            Some("ok")
        );
        assert_eq!(settle_option(&req, PermissionAction::Ask), None);
    }

    #[test]
    fn wire_form_round_trips() {
        for (json, policy) in [
            ("\"ask\"", PermissionPolicy::Ask),
            ("\"allow_all\"", PermissionPolicy::AllowAll),
            ("\"deny_all\"", PermissionPolicy::DenyAll),
            (
                "{\"rules\":[{\"kind\":\"execute\",\"allow\":false}]}",
                PermissionPolicy::Rules(vec![PermissionRule {
                    kind: Some("execute".into()),
                    allow: false,
                }]),
            ),
        ] {
            let parsed: PermissionPolicy = serde_json::from_str(json).unwrap();
            assert_eq!(parsed, policy);
            assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
        }
    }
}
