//! Gateway error type → OpenAI-shaped JSON + HTTP status.

#[cfg(feature = "server")]
use axum::http::StatusCode;
#[cfg(feature = "server")]
use axum::response::{IntoResponse, Response};
#[cfg(feature = "server")]
use axum::Json;
#[cfg(feature = "server")]
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("unknown model alias '{0}'")]
    UnknownModel(String),
    #[error("native feature '{feature}' is not available on route '{route}'")]
    NativeFeatureUnsupported { feature: String, route: String },
    #[error("invalid request: {0}")]
    BadRequest(String),
    #[error("all legs of route '{route}' failed")]
    AllLegsFailed {
        route: String,
        failures: Vec<LegFailure>,
    },
    #[error("all providers for route '{0}' are unavailable")]
    AllCircuitsOpen(String),
    #[error("upstream timed out")]
    UpstreamTimeout,
    #[error("upstream error {status}: {body}")]
    Upstream { status: u16, body: String },
    #[error("request blocked by content policy '{policy}'")]
    ContentBlocked {
        policy: String,
        scanners: Vec<String>,
    },
    #[error("guard classifier for policy '{policy}' is unavailable")]
    GuardUnavailable { policy: String },
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LegFailure {
    pub provider: String,
    pub model: String,
    pub message: String,
}

impl GatewayError {
    #[cfg(feature = "server")]
    pub fn status(&self) -> StatusCode {
        match self {
            GatewayError::UnknownModel(_) => StatusCode::NOT_FOUND,
            GatewayError::NativeFeatureUnsupported { .. } | GatewayError::BadRequest(_) => {
                StatusCode::BAD_REQUEST
            }
            GatewayError::AllLegsFailed { .. } | GatewayError::Upstream { .. } => {
                StatusCode::BAD_GATEWAY
            }
            GatewayError::AllCircuitsOpen(_) => StatusCode::SERVICE_UNAVAILABLE,
            GatewayError::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
            GatewayError::ContentBlocked { .. } => StatusCode::BAD_REQUEST,
            GatewayError::GuardUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    #[cfg(feature = "server")]
    pub(crate) fn code(&self) -> &'static str {
        match self {
            GatewayError::UnknownModel(_) => "model_not_found",
            GatewayError::NativeFeatureUnsupported { .. } => "native_feature_unsupported",
            GatewayError::BadRequest(_) => "invalid_request_error",
            GatewayError::AllLegsFailed { .. } => "all_legs_failed",
            GatewayError::AllCircuitsOpen(_) => "circuit_open",
            GatewayError::UpstreamTimeout => "upstream_timeout",
            GatewayError::Upstream { .. } => "upstream_error",
            GatewayError::ContentBlocked { .. } => "content_blocked",
            GatewayError::GuardUnavailable { .. } => "guard_unavailable",
        }
    }
}

#[cfg(feature = "server")]
impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let mut error = json!({
            "type": self.code(),
            "message": self.to_string(),
            "code": self.code(),
        });
        if let GatewayError::AllLegsFailed { failures, .. } = &self {
            error["failures"] = json!(failures);
        }
        if let GatewayError::ContentBlocked { policy, scanners } = &self {
            error["type"] = json!("content_policy_violation");
            error["message"] = json!(format!(
                "Request blocked by content policy '{policy}' (scanners: {})",
                scanners.join(", ")
            ));
            error["scanners"] = json!(scanners);
        }
        (self.status(), Json(json!({ "error": error }))).into_response()
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn content_blocked_is_bad_request_with_named_scanners() {
        let e = GatewayError::ContentBlocked {
            policy: "strict".into(),
            scanners: vec!["secrets".into(), "role_override".into()],
        };
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
        assert_eq!(e.code(), "content_blocked");
        let msg = e.to_string();
        assert!(msg.contains("strict"), "message names the policy: {msg}");
    }
}
