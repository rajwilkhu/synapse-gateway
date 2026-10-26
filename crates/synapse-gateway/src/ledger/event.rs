//! talos-aligned published usage event (camelCase JSON). Decouples the wire
//! format from the DB-shaped `UsageEntry`. Mirrors the conventions of
//! `talos-core/src/tool_events.rs` (`ToolEvent`): `rename_all = "camelCase"`,
//! tenancy as `namespace`, a `type` discriminator, RFC3339 timestamp.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::ledger::UsageEntry;

/// Marketplace ledger subscription discriminator (`attributes.EventType`).
pub const LEDGER_EVENT_TYPE: &str = "Ledger.LLMTokensConsumed";

/// One terminal usage event, published to a topic per request.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    /// Tenancy. talos calls this `namespace`; synapse maps its `tenant` onto it.
    pub namespace: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// End-user attribution within the namespace (`x-synapse-user`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Conversation / agent thread (`x-synapse-thread`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Chat / work message within the thread (`x-synapse-message`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub request_id: String,
    pub timestamp: DateTime<Utc>,
    #[serde(rename = "type")]
    pub event_type: &'static str,
    pub route: String,
    pub provider: String,
    pub model: String,
    pub lane: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub status: String,
    /// Lane discriminator: "chat" or "embedding".
    pub op: String,
    /// Caller-supplied classification of the work (`x-synapse-user-task-type`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_task_type: Option<String>,
    /// Resolved classification of the work the gateway performed. Always set
    /// (defaults to `"simple"`), so unlike `user_task_type` it is never skipped.
    pub ai_task_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

impl From<&UsageEntry> for UsageEvent {
    fn from(e: &UsageEntry) -> Self {
        Self {
            namespace: e.tenant.clone(),
            workspace: e.workspace.clone(),
            user: e.user.clone(),
            thread_id: e.thread.clone(),
            message_id: e.message.clone(),
            request_id: e.request_id.clone(),
            timestamp: e.ts,
            event_type: "usage",
            route: e.route.clone(),
            provider: e.provider.clone(),
            model: e.model.clone(),
            lane: e.lane.clone(),
            input_tokens: e.input_tokens,
            output_tokens: e.output_tokens,
            cost_usd: e.cost_usd,
            status: e.status.clone(),
            op: e.op.clone(),
            user_task_type: e.user_task_type.clone(),
            ai_task_type: e.ai_task_type.clone(),
            score: e.score,
            confidence: e.confidence,
            tier: e.tier.clone(),
            outcome: e.outcome.clone(),
        }
    }
}

impl UsageEvent {
    /// Broker message attributes for subscription filtering (talos-aligned keys
    /// plus synapse-useful `provider`/`status`). Returned as ordered pairs so
    /// both backends can convert to their SDK-specific attribute type.
    pub fn attributes(&self) -> Vec<(&'static str, String)> {
        vec![
            ("EventType", LEDGER_EVENT_TYPE.to_string()),
            ("namespace", self.namespace.clone()),
            ("requestId", self.request_id.clone()),
            ("type", self.event_type.to_string()),
            ("provider", self.provider.clone()),
            ("status", self.status.clone()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn entry() -> UsageEntry {
        UsageEntry {
            ts: Utc.with_ymd_and_hms(2026, 6, 10, 15, 30, 45).unwrap(),
            tenant: "acme".into(),
            workspace: None,
            user: None,
            thread: None,
            message: None,
            route: "gemini-pro".into(),
            provider: "vertex".into(),
            model: "gemini-3-pro".into(),
            lane: "standard".into(),
            input_tokens: 3,
            output_tokens: 5,
            cost_usd: 0.001,
            request_id: "req-1".into(),
            status: "ok".into(),
            op: "chat".into(),
            user_task_type: None,
            ai_task_type: "simple".into(),
            score: None,
            confidence: None,
            tier: None,
            outcome: None,
        }
    }

    #[test]
    fn serializes_route_decision_receipt_fields_when_present() {
        let mut e = entry();
        e.op = "route_decision".into();
        e.score = Some(1.75);
        e.confidence = Some(0.91);
        e.tier = Some("hard".into());
        e.outcome = Some("decided".into());
        let v = serde_json::to_value(UsageEvent::from(&e)).unwrap();
        assert_eq!(v["op"], "route_decision");
        assert_eq!(v["score"], 1.75);
        assert_eq!(v["confidence"], 0.91);
        assert_eq!(v["tier"], "hard");
        assert_eq!(v["outcome"], "decided");
    }

    #[test]
    fn serializes_talos_aligned_camelcase() {
        let v = serde_json::to_value(UsageEvent::from(&entry())).unwrap();
        assert_eq!(v["namespace"], "acme");
        assert_eq!(v["type"], "usage");
        assert_eq!(v["requestId"], "req-1");
        assert_eq!(v["inputTokens"], 3);
        assert_eq!(v["outputTokens"], 5);
        assert_eq!(v["costUsd"], 0.001);
        assert_eq!(v["lane"], "standard");
        assert_eq!(v["op"], "chat");
        assert!(v.get("workspace").is_none());
        assert!(v.get("user").is_none());
        assert!(v.get("request_id").is_none());
        assert!(v.get("input_tokens").is_none());
        assert!(v.get("userTaskType").is_none());
        // Always resolved, so unlike userTaskType it is always serialized.
        assert_eq!(v["aiTaskType"], "simple");
        assert!(v.get("ai_task_type").is_none());
    }

    #[test]
    fn serializes_the_resolved_ai_task_type() {
        let mut e = entry();
        e.ai_task_type = "conversation".into();
        let v = serde_json::to_value(UsageEvent::from(&e)).unwrap();
        assert_eq!(v["aiTaskType"], "conversation");
    }

    #[test]
    fn serializes_user_task_type_when_present() {
        let mut e = entry();
        e.user_task_type = Some("summarisation".into());
        let v = serde_json::to_value(UsageEvent::from(&e)).unwrap();
        assert_eq!(v["userTaskType"], "summarisation");
        assert!(v.get("user_task_type").is_none());
    }

    #[test]
    fn serializes_thread_and_message_when_present() {
        let mut e = entry();
        e.thread = Some("thread-9".into());
        e.message = Some("msg-7".into());
        let v = serde_json::to_value(UsageEvent::from(&e)).unwrap();
        assert_eq!(v["threadId"], "thread-9");
        assert_eq!(v["messageId"], "msg-7");
    }

    #[test]
    fn serializes_user_when_present() {
        let mut e = entry();
        e.user = Some("user-42".into());
        let v = serde_json::to_value(UsageEvent::from(&e)).unwrap();
        assert_eq!(v["user"], "user-42");
    }

    #[test]
    fn serializes_embedding_op() {
        let mut e = entry();
        e.op = "embedding".into();
        let v = serde_json::to_value(UsageEvent::from(&e)).unwrap();
        assert_eq!(v["op"], "embedding");
    }

    #[test]
    fn attributes_have_talos_keys() {
        let attrs = UsageEvent::from(&entry()).attributes();
        let keys: Vec<&str> = attrs.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![
                "EventType",
                "namespace",
                "requestId",
                "type",
                "provider",
                "status"
            ]
        );
        assert_eq!(attrs[0].1, LEDGER_EVENT_TYPE);
        assert_eq!(attrs[1].1, "acme");
    }
}
