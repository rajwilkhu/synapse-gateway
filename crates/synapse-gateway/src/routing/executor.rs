//! Standard-lane executor: walk a route's legs with per-leg breaker + retry.

use std::sync::Arc;

use tap::Pipe;

use crate::error::{GatewayError, LegFailure};
use crate::providers::genai_provider::Provider;
use crate::providers::Catalog;
use crate::resilience::{run_with_classifier, ResilienceError};
use crate::routing::effort::Effort;
use crate::routing::request::ChatRequest;
use crate::routing::stream::{Accumulator, FinishReason, StreamItem, ToolCallOut};
use crate::routing::table::ChainLeg;
use futures::stream::{BoxStream, Stream, StreamExt};

/// Normalised result of one completed LLM call.
#[derive(Debug, Clone)]
pub struct Completion {
    pub provider: String,
    pub model: String,
    pub content: String,
    pub tool_calls: Vec<ToolCallOut>,
    pub finish_reason: FinishReason,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Build a genai `ChatRequest` from the gateway request: messages (incl. tool
/// calls / tool results) plus tool definitions. `tool_choice` is not expressible
/// on genai 0.6 `ChatRequest` and is dropped on this lane (documented).
fn to_genai_request(req: &ChatRequest) -> genai::chat::ChatRequest {
    use genai::chat::{ChatMessage, Tool, ToolCall, ToolResponse};

    let mut chat = genai::chat::ChatRequest::default();

    for m in &req.messages {
        match m.role.as_str() {
            "assistant" if m.tool_calls.is_some() => {
                let calls: Vec<ToolCall> = m
                    .tool_calls
                    .as_ref()
                    .unwrap()
                    .iter()
                    .filter_map(openai_tool_call_to_genai)
                    .collect();
                chat = chat.append_message(ChatMessage::from(calls));
            }
            "tool" => {
                let call_id = m.tool_call_id.clone().unwrap_or_default();
                let content = m
                    .content
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| m.content.to_string());
                chat = chat.append_message(ChatMessage::from(ToolResponse { call_id, content }));
            }
            role => {
                let msg = match role {
                    "system" => ChatMessage::system(genai::chat::MessageContent::from_parts(
                        crate::routing::content_parts::content_to_genai_parts(&m.content),
                    )),
                    "assistant" => ChatMessage::assistant(genai::chat::MessageContent::from_parts(
                        crate::routing::content_parts::content_to_genai_parts(&m.content),
                    )),
                    _ => ChatMessage::user(genai::chat::MessageContent::from_parts(
                        crate::routing::content_parts::content_to_genai_parts(&m.content),
                    )),
                };
                chat = chat.append_message(msg);
            }
        }
    }

    if let Some(tools) = &req.tools {
        let mapped: Vec<Tool> = tools.iter().filter_map(openai_tool_to_genai).collect();
        if !mapped.is_empty() {
            chat = chat.with_tools(mapped);
        }
    }

    chat
}

/// `{type:function, function:{name, description, parameters}}` -> genai `Tool`.
fn openai_tool_to_genai(v: &serde_json::Value) -> Option<genai::chat::Tool> {
    let f = v.get("function")?;
    let name = f.get("name")?.as_str()?.to_string();
    let mut tool = genai::chat::Tool::new(name);
    if let Some(desc) = f.get("description").and_then(|d| d.as_str()) {
        tool = tool.with_description(desc);
    }
    if let Some(params) = f.get("parameters") {
        tool = tool.with_schema(params.clone());
    }
    Some(tool)
}

/// `{id, function:{name, arguments}}` -> genai `ToolCall`. `arguments` is an
/// OpenAI JSON STRING; parse to Value (fall back to a string Value if not JSON).
fn openai_tool_call_to_genai(v: &serde_json::Value) -> Option<genai::chat::ToolCall> {
    let call_id = v.get("id")?.as_str()?.to_string();
    let f = v.get("function")?;
    let fn_name = f.get("name")?.as_str()?.to_string();
    let raw_args = f.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
    let fn_arguments =
        serde_json::from_str(raw_args).unwrap_or_else(|_| serde_json::json!(raw_args));
    Some(genai::chat::ToolCall {
        call_id,
        fn_name,
        fn_arguments,
        thought_signatures: None,
    })
}

fn to_genai_options(req: &ChatRequest, effort: Option<Effort>) -> genai::chat::ChatOptions {
    genai::chat::ChatOptions::default()
        .pipe(|o| match req.temperature {
            Some(t) => o.with_temperature(t as f64),
            None => o,
        })
        .pipe(|o| match &req.response_format {
            Some(rf) => match rf.kind.as_str() {
                "json_object" => o.with_response_format(genai::chat::ChatResponseFormat::JsonMode),
                "json_schema" => {
                    if let Some(spec) = rf.json_schema.clone() {
                        o.with_response_format(genai::chat::ChatResponseFormat::JsonSpec(
                            genai::chat::JsonSpec::new("synapse", spec),
                        ))
                    } else {
                        o
                    }
                }
                _ => o,
            },
            None => o,
        })
        .pipe(
            |o| match client_effort(req).or(effort).and_then(Effort::to_genai) {
                Some(e) => o.with_reasoning_effort(e),
                None => o,
            },
        )
}

/// A client-supplied OpenAI `reasoning_effort` (captured in `passthrough`).
pub(crate) fn client_effort(req: &ChatRequest) -> Option<Effort> {
    req.passthrough
        .get("reasoning_effort")
        .and_then(Effort::from_value)
}

/// True if a genai error is worth advancing the chain for (transient/5xx/timeout).
/// 4xx (auth, bad request) are NOT retryable — abort the chain immediately.
///
/// Implementation note: we use STRUCTURED matching on the genai error enum rather than
/// string inspection. The previous implementation checked for `" 500"`, `" 502"`, etc.
/// (space before digits), but genai's `webc::Error::ResponseFailedStatus` Display format
/// is `"Request failed with status code '503 ...'` — digits are preceded by a single-quote,
/// not a space — so the old checks never matched and 5xx errors were treated as non-retryable,
/// causing `execute_chain` to break out of the fallback loop instead of advancing to the
/// next leg.
///
/// Structured approach: match the two web-call wrapper variants
/// (`WebModelCall` / `WebAdapterCall`) that carry a `genai::webc::Error`, then match
/// `ResponseFailedStatus { status, .. }` and call `status.is_server_error()` on the
/// typed `reqwest::StatusCode`. Timeout and connection errors are detected via
/// `webc::Error::Reqwest(e)` with `e.is_timeout() || e.is_connect()`.
/// `genai::Error::HttpError { status, .. }` is also matched for completeness.
fn is_genai_retryable(e: &genai::Error) -> bool {
    /// Align with `resilience::is_retryable_reqwest`: 5xx, 429, request timeout.
    fn status_retryable(status: reqwest::StatusCode) -> bool {
        status.is_server_error()
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status == reqwest::StatusCode::REQUEST_TIMEOUT
    }

    /// Check whether a `genai::webc::Error` represents a transient/server error.
    fn webc_retryable(we: &genai::webc::Error) -> bool {
        match we {
            genai::webc::Error::ResponseFailedStatus { status, .. } => status_retryable(*status),
            genai::webc::Error::Reqwest(re) => re.is_timeout() || re.is_connect(),
            _ => false,
        }
    }

    match e {
        genai::Error::WebModelCall { webc_error, .. } => webc_retryable(webc_error),
        genai::Error::WebAdapterCall { webc_error, .. } => webc_retryable(webc_error),
        genai::Error::HttpError { status, .. } => status_retryable(*status),
        genai::Error::WebStream { error, .. } => {
            if let Some(inner) = error.downcast_ref::<genai::Error>() {
                is_genai_retryable(inner)
            } else if let Some(re) = error.downcast_ref::<reqwest::Error>() {
                re.is_timeout() || re.is_connect() || re.status().is_some_and(status_retryable)
            } else {
                false
            }
        }
        _ => false,
    }
}

fn genai_error_status(e: &genai::Error) -> Option<u16> {
    fn webc_status(we: &genai::webc::Error) -> Option<u16> {
        match we {
            genai::webc::Error::ResponseFailedStatus { status, .. } => Some(status.as_u16()),
            genai::webc::Error::Reqwest(re) => re.status().map(|s| s.as_u16()),
            _ => None,
        }
    }
    match e {
        genai::Error::WebModelCall { webc_error, .. }
        | genai::Error::WebAdapterCall { webc_error, .. } => webc_status(webc_error),
        genai::Error::HttpError { status, .. } => Some(status.as_u16()),
        genai::Error::WebStream { error, .. } => {
            if let Some(inner) = error.downcast_ref::<genai::Error>() {
                genai_error_status(inner)
            } else if let Some(re) = error.downcast_ref::<reqwest::Error>() {
                re.status().map(|s| s.as_u16())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn genai_error_kind(e: &genai::Error) -> &'static str {
    fn webc_kind(we: &genai::webc::Error) -> &'static str {
        match we {
            genai::webc::Error::ResponseFailedStatus { .. } => "http_status",
            genai::webc::Error::Reqwest(re) if re.is_timeout() => "timeout",
            genai::webc::Error::Reqwest(re) if re.is_connect() => "connect",
            genai::webc::Error::Reqwest(_) => "transport",
            _ => "webc_error",
        }
    }
    match e {
        genai::Error::WebModelCall { webc_error, .. }
        | genai::Error::WebAdapterCall { webc_error, .. } => webc_kind(webc_error),
        genai::Error::HttpError { .. } => "http_status",
        genai::Error::WebStream { error, .. } => {
            if let Some(inner) = error.downcast_ref::<genai::Error>() {
                genai_error_kind(inner)
            } else if let Some(re) = error.downcast_ref::<reqwest::Error>() {
                if re.is_timeout() {
                    "timeout"
                } else if re.is_connect() {
                    "connect"
                } else {
                    "transport"
                }
            } else {
                "web_stream"
            }
        }
        _ => "genai_error",
    }
}

/// Errors from a single streaming leg. `Start` failures are fallback-eligible.
#[derive(Debug)]
pub enum LegError {
    /// Failed before/at stream start (connection, 5xx, etc.).
    Start(String),
    /// Failed after items began flowing.
    MidStream(String),
}

impl std::fmt::Display for LegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LegError::Start(s) => write!(f, "start: {s}"),
            LegError::MidStream(s) => write!(f, "mid-stream: {s}"),
        }
    }
}

/// Map a genai `StopReason` to our `FinishReason`.
fn map_stop_reason(sr: Option<&genai::chat::StopReason>) -> FinishReason {
    match sr {
        Some(genai::chat::StopReason::ToolCall(_)) => FinishReason::ToolCalls,
        Some(genai::chat::StopReason::MaxTokens(_)) => FinishReason::Length,
        _ => FinishReason::Stop,
    }
}

/// Stateful call_id -> stable 0-based index assignment for tool-call streaming.
#[derive(Default)]
struct ToolIndexer {
    ids: Vec<String>,
}
impl ToolIndexer {
    /// Returns (index, is_first_time_seen).
    fn index_of(&mut self, call_id: &str) -> (u32, bool) {
        if let Some(pos) = self.ids.iter().position(|c| c == call_id) {
            (pos as u32, false)
        } else {
            self.ids.push(call_id.to_string());
            ((self.ids.len() - 1) as u32, true)
        }
    }
}

/// State threaded across a genai event stream.
struct St<S> {
    inner: S,
    indexer: ToolIndexer,
    input_tokens: u64,
    output_tokens: u64,
}

async fn next_genai_item<S>(st: &mut St<S>) -> Option<Result<StreamItem, genai::Error>>
where
    S: Stream<Item = Result<genai::chat::ChatStreamEvent, genai::Error>> + Unpin,
{
    loop {
        match st.inner.next().await {
            None => return None,
            Some(Err(e)) => return Some(Err(e)),
            Some(Ok(ev)) => match ev {
                genai::chat::ChatStreamEvent::Chunk(c) => {
                    if !c.content.is_empty() {
                        return Some(Ok(StreamItem::Delta(c.content)));
                    }
                }
                genai::chat::ChatStreamEvent::ToolCallChunk(tc) => {
                    let call = tc.tool_call;
                    let (index, first) = st.indexer.index_of(&call.call_id);
                    let args = match &call.fn_arguments {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    return Some(Ok(StreamItem::ToolCallDelta {
                        index,
                        id: if first { Some(call.call_id) } else { None },
                        name: if first { Some(call.fn_name) } else { None },
                        args_fragment: args,
                    }));
                }
                genai::chat::ChatStreamEvent::End(end) => {
                    if let Some(u) = &end.captured_usage {
                        st.input_tokens = u.prompt_tokens.unwrap_or(0).max(0) as u64;
                        st.output_tokens = u.completion_tokens.unwrap_or(0).max(0) as u64;
                    }
                    let done = StreamItem::Done {
                        input_tokens: st.input_tokens,
                        output_tokens: st.output_tokens,
                        finish_reason: map_stop_reason(end.captured_stop_reason.as_ref()),
                    };
                    return Some(Ok(done));
                }
                _ => {} // Start / ReasoningChunk / ThoughtSignatureChunk ignored; loop for next
            },
        }
    }
}

fn normalized_stream<S>(state: St<S>) -> impl Stream<Item = Result<StreamItem, LegError>>
where
    S: Stream<Item = Result<genai::chat::ChatStreamEvent, genai::Error>> + Unpin,
{
    futures::stream::unfold(state, |mut st| async move {
        next_genai_item(&mut st)
            .await
            .map(|r| (r.map_err(|e| LegError::MidStream(e.to_string())), st))
    })
}

/// Standard-lane per-leg primitive: open a genai stream, normalize to `StreamItem`.
/// Outer `Err(LegError::Start)` = stream could not begin (fallback-eligible).
pub async fn stream_one_leg_standard(
    provider: &Arc<Provider>,
    model: &str,
    req: &ChatRequest,
    effort: Option<Effort>,
) -> Result<impl Stream<Item = Result<StreamItem, LegError>>, LegError> {
    let chat_req = to_genai_request(req);
    let opts = to_genai_options(req, effort)
        .with_capture_usage(true)
        .with_capture_tool_calls(true);

    let resp = provider
        .client
        .exec_chat_stream(model.to_string(), chat_req, Some(&opts))
        .await
        .map_err(|e| LegError::Start(e.to_string()))?;

    let state = St {
        inner: Box::pin(resp.stream),
        indexer: ToolIndexer::default(),
        input_tokens: 0,
        output_tokens: 0,
    };

    Ok(normalized_stream(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(body: serde_json::Value) -> ChatRequest {
        serde_json::from_value(body).unwrap()
    }

    #[test]
    fn maps_tools_into_genai_request() {
        let r = req(serde_json::json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "function", "function": {"name": "get_weather",
                "description": "Lookup", "parameters": {"type": "object"}}}]
        }));
        let g = to_genai_request(&r);
        let tools = g.tools.expect("tools mapped");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name.to_string(), "get_weather");
        assert_eq!(tools[0].description.as_deref(), Some("Lookup"));
    }

    #[test]
    fn maps_assistant_tool_calls_and_tool_results() {
        let r = req(serde_json::json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": "weather?"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_0", "type": "function", "function": {"name": "f", "arguments": "{\"c\":\"SF\"}"}}]},
                {"role": "tool", "tool_call_id": "call_0", "content": "21C"}
            ]
        }));
        let g = to_genai_request(&r);
        assert_eq!(g.messages.len(), 3);
    }

    use crate::providers::genai_provider::{build_openai_compat_provider, OpenAiCompatConfig};
    use crate::routing::stream::{FinishReason, StreamItem};
    use futures::StreamExt;
    use std::time::Duration;

    #[tokio::test]
    async fn standard_lane_streams_text_then_done() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // OpenAI-style SSE: two content deltas, a finish, then [DONE].
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"He\"}}]}\n\n\
                   data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n\
                   data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n\
                   data: [DONE]\n\n";
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse),
            )
            .mount(&mock)
            .await;

        let provider = Arc::new(
            build_openai_compat_provider(
                "oai",
                OpenAiCompatConfig {
                    base_url: format!("{}/v1", mock.uri()),
                    api_key: "k".into(),
                    request_timeout: Duration::from_secs(5),
                    endpoint_override: None,
                },
            )
            .unwrap(),
        );

        let req = req(
            serde_json::json!({"model":"m","messages":[{"role":"user","content":"hi"}],"stream":true}),
        );
        let mut stream = std::pin::pin!(stream_one_leg_standard(&provider, "m", &req, None)
            .await
            .expect("stream starts"));
        let mut items = Vec::new();
        while let Some(it) = stream.next().await {
            items.push(it.expect("no mid-stream error"));
        }
        assert!(items
            .iter()
            .any(|i| matches!(i, StreamItem::Delta(t) if t == "He")));
        assert!(matches!(
            items.last().unwrap(),
            StreamItem::Done {
                input_tokens: 3,
                output_tokens: 2,
                finish_reason: FinishReason::Stop
            }
        ));
    }

    #[tokio::test]
    async fn execute_buffered_falls_back_on_midstream_failure() {
        use crate::providers::Catalog;
        use crate::routing::table::ChainLeg;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Leg 1: stream starts then the body is cut after one delta (no Done) -> mid-stream failure.
        let bad = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"par\"}}]}\n\n"),
            ) // no [DONE]/usage
            .mount(&bad)
            .await;
        // Leg 2: clean stream.
        let good = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n\
                         data: [DONE]\n\n",
                    ),
            )
            .mount(&good)
            .await;

        let catalog = Catalog::for_test(vec![
            ("p1", format!("{}/v1", bad.uri())),
            ("p2", format!("{}/v1", good.uri())),
        ]);
        let legs = vec![
            ChainLeg {
                provider: "p1".into(),
                model: "m".into(),
                ..Default::default()
            },
            ChainLeg {
                provider: "p2".into(),
                model: "m".into(),
                ..Default::default()
            },
        ];
        let r =
            req(serde_json::json!({"model":"route","messages":[{"role":"user","content":"hi"}]}));
        let c = execute_buffered(&catalog, "route", &legs, &r)
            .await
            .unwrap();
        assert_eq!(c.content, "ok");
        assert_eq!(c.provider, "p2");
    }

    #[tokio::test]
    async fn execute_streaming_commits_first_leg_with_items() {
        use crate::providers::Catalog;
        use crate::routing::stream::StreamItem;
        use crate::routing::table::ChainLeg;
        use futures::StreamExt;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Leg 1 fails to start (500) -> fall back. Leg 2 streams.
        let bad = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&bad)
            .await;
        let good = MockServer::start().await;
        Mock::given(method("POST")).and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"go\"}}]}\n\n\
                                  data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n\
                                  data: [DONE]\n\n"))
            .mount(&good).await;

        let catalog = Catalog::for_test(vec![
            ("p1", format!("{}/v1", bad.uri())),
            ("p2", format!("{}/v1", good.uri())),
        ]);
        let legs = vec![
            ChainLeg {
                provider: "p1".into(),
                model: "m".into(),
                ..Default::default()
            },
            ChainLeg {
                provider: "p2".into(),
                model: "m".into(),
                ..Default::default()
            },
        ];
        let r = req(
            serde_json::json!({"model":"route","messages":[{"role":"user","content":"hi"}],"stream":true}),
        );
        let committed = execute_streaming(&catalog, "route", &legs, &r)
            .await
            .unwrap();
        assert_eq!(committed.provider, "p2");
        let mut stream = committed.stream;
        let mut items = Vec::new();
        while let Some(i) = stream.next().await {
            items.push(i.unwrap());
        }
        assert!(items
            .iter()
            .any(|i| matches!(i, StreamItem::Delta(t) if t == "go")));
    }

    #[tokio::test]
    async fn first_chunk_timeout_falls_back() {
        use crate::providers::Catalog;
        use crate::routing::table::ChainLeg;
        use std::time::Duration;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Leg 1: the entire response (including response head) is delayed 400ms.
        // With a 150ms first-chunk timeout applied to stream.next(), leg 1 is
        // abandoned before any item arrives and the chain falls back to leg 2.
        //
        // Note: wiremock's set_delay delays BEFORE the response head is sent,
        // so exec_chat_stream's connection .await will block for 400ms. The
        // stream returned by stream_one_leg_standard won't yield its first item
        // within that window. The first-chunk timeout in buffer_one_leg_timed
        // wraps stream.next() — it fires at 150ms, classifying the failure as
        // LegError::Start("first-chunk timeout"), which is fallback-eligible.
        let slow = MockServer::start().await;
        Mock::given(method("POST")).and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_delay(Duration::from_millis(400))
                .set_body_string("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{}}\n\ndata: [DONE]\n\n"))
            .mount(&slow).await;
        let good = MockServer::start().await;
        Mock::given(method("POST")).and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                                  data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n\
                                  data: [DONE]\n\n")).mount(&good).await;

        let catalog = Catalog::for_test(vec![
            ("p1", format!("{}/v1", slow.uri())),
            ("p2", format!("{}/v1", good.uri())),
        ]);
        let legs = vec![
            ChainLeg {
                provider: "p1".into(),
                model: "m".into(),
                ..Default::default()
            },
            ChainLeg {
                provider: "p2".into(),
                model: "m".into(),
                ..Default::default()
            },
        ];
        let r =
            req(serde_json::json!({"model":"route","messages":[{"role":"user","content":"hi"}]}));
        let timeouts = StreamTimeouts {
            first_chunk: Duration::from_millis(150),
            idle: Duration::from_secs(5),
        };
        let c = execute_buffered_with_timeouts(&catalog, "route", &legs, &r, timeouts)
            .await
            .unwrap();
        assert_eq!(c.provider, "p2");
    }

    fn opts_req(extra: serde_json::Value) -> ChatRequest {
        let mut body = serde_json::json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}]
        });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap_or_default());
        serde_json::from_value(body).unwrap()
    }

    fn effort_name(o: &genai::chat::ChatOptions) -> Option<&'static str> {
        o.reasoning_effort.as_ref().map(|e| e.variant_name())
    }

    #[test]
    fn leg_effort_becomes_reasoning_effort() {
        let o = to_genai_options(
            &opts_req(serde_json::json!({})),
            Some(crate::routing::effort::Effort::Medium),
        );
        assert_eq!(effort_name(&o), Some("medium"));
    }

    #[test]
    fn effort_none_sends_no_reasoning_effort() {
        let o = to_genai_options(
            &opts_req(serde_json::json!({})),
            Some(crate::routing::effort::Effort::None),
        );
        assert_eq!(effort_name(&o), None);
    }

    #[test]
    fn client_reasoning_effort_is_forwarded() {
        let o = to_genai_options(
            &opts_req(serde_json::json!({"reasoning_effort": "high"})),
            None,
        );
        assert_eq!(effort_name(&o), Some("high"));
    }

    #[test]
    fn client_reasoning_effort_wins_over_leg_effort() {
        let o = to_genai_options(
            &opts_req(serde_json::json!({"reasoning_effort": "low"})),
            Some(crate::routing::effort::Effort::Max),
        );
        assert_eq!(effort_name(&o), Some("low"));
    }

    #[test]
    fn client_none_is_not_forwarded_and_leg_effort_does_not_replace_it() {
        let o = to_genai_options(
            &opts_req(serde_json::json!({"reasoning_effort": "none"})),
            Some(Effort::High),
        );
        assert_eq!(effort_name(&o), None);
    }

    #[test]
    fn unparseable_client_effort_falls_back_to_leg_effort() {
        let o = to_genai_options(
            &opts_req(serde_json::json!({"reasoning_effort": "extreme"})),
            Some(Effort::Low),
        );
        assert_eq!(effort_name(&o), Some("low"));
    }

    #[test]
    fn no_effort_anywhere_sends_nothing() {
        assert_eq!(
            effort_name(&to_genai_options(&opts_req(serde_json::json!({})), None)),
            None
        );
    }

    #[tokio::test]
    async fn execute_buffered_retries_transient_503_before_succeeding() {
        use crate::providers::Catalog;
        use crate::routing::table::ChainLeg;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
            .up_to_n_times(1)
            .expect(1)
            .mount(&srv)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"recovered\"}}]}\n\n\
                         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n\
                         data: [DONE]\n\n",
                    ),
            )
            .expect(1)
            .mount(&srv)
            .await;

        let catalog = Catalog::for_test(vec![("p1", format!("{}/v1", srv.uri()))]);
        #[cfg(feature = "server")]
        let exporter = {
            let (m, exporter) = crate::telemetry::test_metrics();
            catalog.attach_metrics(&m);
            exporter
        };
        let legs = vec![ChainLeg {
            provider: "p1".into(),
            model: "m".into(),
            ..Default::default()
        }];
        let r =
            req(serde_json::json!({"model":"route","messages":[{"role":"user","content":"hi"}]}));
        let c = execute_buffered(&catalog, "route", &legs, &r)
            .await
            .unwrap();
        assert_eq!(c.content, "recovered");
        assert_eq!(c.provider, "p1");
        #[cfg(feature = "server")]
        {
            let text = crate::telemetry::scrape(&exporter);
            assert!(
                text.lines()
                    .any(|l| l == r#"synapse_resilience_retry_attempts_total{label="p1"} 1"#),
                "expected retry metric in:\n{text}"
            );
        }
    }

    #[tokio::test]
    async fn execute_buffered_opens_circuit_breaker_and_returns_all_circuits_open() {
        use crate::providers::Catalog;
        use crate::routing::table::ChainLeg;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let srv = MockServer::start().await;
        // Return 400 (non-retryable, so 1 HTTP call per execute_buffered invocation)
        // 5 times to trip the Profile::Default (breaker_threshold=5) breaker; the
        // 6th call must be short-circuited without hitting wiremock.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad"))
            .expect(5)
            .mount(&srv)
            .await;

        let catalog = Catalog::for_test(vec![("p1", format!("{}/v1", srv.uri()))]);
        let legs = vec![ChainLeg {
            provider: "p1".into(),
            model: "m".into(),
            ..Default::default()
        }];
        let r =
            req(serde_json::json!({"model":"route","messages":[{"role":"user","content":"hi"}]}));

        for _ in 0..5 {
            assert!(matches!(
                execute_buffered(&catalog, "route", &legs, &r).await,
                Err(GatewayError::AllLegsFailed { .. })
            ));
        }
        // Sixth call: breaker is open, so AllCircuitsOpen is returned with no 6th HTTP call.
        assert!(matches!(
            execute_buffered(&catalog, "route", &legs, &r).await,
            Err(GatewayError::AllCircuitsOpen(ref route)) if route == "route"
        ));
    }

    #[tokio::test]
    async fn execute_buffered_and_streaming_emit_fallback_logs() {
        use crate::providers::Catalog;
        use crate::routing::table::ChainLeg;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        crate::guard::classifier::tests::capture_logs();

        let bad = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_string("secret_prompt_echo"))
            .mount(&bad)
            .await;
        let good = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n\
                         data: [DONE]\n\n",
                    ),
            )
            .mount(&good)
            .await;

        let catalog = Catalog::for_test(vec![
            ("p1", format!("{}/v1", bad.uri())),
            ("p2", format!("{}/v1", good.uri())),
        ]);
        let legs = vec![
            ChainLeg {
                provider: "p1".into(),
                model: "m1".into(),
                ..Default::default()
            },
            ChainLeg {
                provider: "p2".into(),
                model: "m2".into(),
                ..Default::default()
            },
        ];
        let _ = crate::guard::classifier::tests::take_logs();

        let r =
            req(serde_json::json!({"model":"route","messages":[{"role":"user","content":"hi"}]}));
        let _ = execute_buffered(&catalog, "route", &legs, &r)
            .await
            .unwrap();
        let _ = execute_streaming(&catalog, "route", &legs, &r)
            .await
            .unwrap();

        let logs = crate::guard::classifier::tests::take_logs();
        let fallback_lines: Vec<_> = logs
            .lines()
            .filter(|l| {
                l.contains("synapse::routing") && l.contains("leg failed; falling back to next leg")
            })
            .collect();
        assert_eq!(fallback_lines.len(), 2, "logs:\n{logs}");
        for line in fallback_lines {
            assert!(
                line.contains(r#"route="route" provider=p1 model=m1 next_provider=p2 next_model=m2 error.kind="http_status" http.status=Some(400)"#),
                "unexpected fallback line: {line}"
            );
            assert!(
                !line.contains("secret_prompt_echo"),
                "synapse fallback logs must not leak upstream error bodies: {line}"
            );
        }
    }
}

async fn run_one_leg(
    provider: &Arc<Provider>,
    leg: &ChainLeg,
    req: &ChatRequest,
) -> Result<Completion, ResilienceError<genai::Error>> {
    let client = provider.client.clone();
    let model = leg.model.clone();
    let chat_req = to_genai_request(req);
    let opts = to_genai_options(req, leg.effort);

    let resp = run_with_classifier(
        move || {
            let (client, model, chat_req, opts) = (
                client.clone(),
                model.clone(),
                chat_req.clone(),
                opts.clone(),
            );
            async move { client.exec_chat(model, chat_req, Some(&opts)).await }
        },
        provider.profile,
        &provider.breaker,
        provider.label,
        is_genai_retryable,
    )
    .await?;

    let content = resp.first_text().unwrap_or_default().to_string();
    let usage = &resp.usage;
    Ok(Completion {
        provider: leg.provider.clone(),
        model: leg.model.clone(),
        content,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        input_tokens: usage.prompt_tokens.unwrap_or(0).max(0) as u64,
        output_tokens: usage.completion_tokens.unwrap_or(0).max(0) as u64,
    })
}

fn log_leg_fallback(
    route: &str,
    leg: &ChainLeg,
    next: &ChainLeg,
    kind: &'static str,
    status: Option<u16>,
) {
    tracing::warn!(
        target: "synapse::routing",
        route,
        provider = %leg.provider,
        model = %leg.model,
        next_provider = %next.provider,
        next_model = %next.model,
        error.kind = kind,
        http.status = ?status,
        "leg failed; falling back to next leg"
    );
}

/// Walk legs in order. Retryable failure (or open breaker) advances; the first
/// non-retryable failure aborts. Returns `AllLegsFailed` if every leg fails.
pub async fn execute_chain(
    catalog: &Catalog,
    route_name: &str,
    legs: &[ChainLeg],
    req: &ChatRequest,
) -> Result<Completion, GatewayError> {
    let mut failures: Vec<LegFailure> = Vec::new();
    let mut all_circuit_open = true;
    for (i, leg) in legs.iter().enumerate() {
        let provider = catalog.get(&leg.provider).ok_or_else(|| {
            GatewayError::BadRequest(format!(
                "route '{route_name}' references unbuilt provider '{}'",
                leg.provider
            ))
        })?;
        match run_one_leg(provider, leg, req).await {
            Ok(c) => return Ok(c),
            Err(ResilienceError::CircuitOpen { name }) => {
                if let Some(next) = legs.get(i + 1) {
                    log_leg_fallback(route_name, leg, next, "circuit_open", None);
                }
                failures.push(LegFailure {
                    provider: leg.provider.clone(),
                    model: leg.model.clone(),
                    message: format!("circuit open: {name}"),
                });
            }
            Err(ResilienceError::Exhausted(e)) => {
                all_circuit_open = false;
                let retryable = is_genai_retryable(&e);
                let kind = genai_error_kind(&e);
                let status = genai_error_status(&e);
                failures.push(LegFailure {
                    provider: leg.provider.clone(),
                    model: leg.model.clone(),
                    message: e.to_string(),
                });
                if !retryable {
                    break; // non-retryable: abort the chain
                }
                if let Some(next) = legs.get(i + 1) {
                    log_leg_fallback(route_name, leg, next, kind, status);
                }
            }
        }
    }
    if all_circuit_open && !failures.is_empty() {
        return Err(GatewayError::AllCircuitsOpen(route_name.to_string()));
    }
    Err(GatewayError::AllLegsFailed {
        route: route_name.to_string(),
        failures,
    })
}

/// Time bounds for a streaming leg.
#[derive(Debug, Clone, Copy)]
pub struct StreamTimeouts {
    /// Max time to the first item (time-to-first-token).
    pub first_chunk: std::time::Duration,
    /// Max gap between successive items.
    pub idle: std::time::Duration,
}

impl Default for StreamTimeouts {
    fn default() -> Self {
        Self {
            first_chunk: std::time::Duration::from_secs(120),
            idle: std::time::Duration::from_secs(60),
        }
    }
}

enum StartError {
    Genai(genai::Error),
    FirstChunkTimeout,
    EmptyStream,
}

impl std::fmt::Debug for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Genai(e) => match genai_error_status(e) {
                Some(status) => write!(f, "{}({status})", genai_error_kind(e)),
                None => write!(f, "{}", genai_error_kind(e)),
            },
            StartError::FirstChunkTimeout => write!(f, "first_chunk_timeout"),
            StartError::EmptyStream => write!(f, "empty_stream"),
        }
    }
}

impl StartError {
    fn is_retryable(&self) -> bool {
        match self {
            StartError::Genai(e) => is_genai_retryable(e),
            StartError::FirstChunkTimeout | StartError::EmptyStream => false,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            StartError::Genai(e) => genai_error_kind(e),
            StartError::FirstChunkTimeout => "first_chunk_timeout",
            StartError::EmptyStream => "empty_stream",
        }
    }

    fn status(&self) -> Option<u16> {
        match self {
            StartError::Genai(e) => genai_error_status(e),
            StartError::FirstChunkTimeout | StartError::EmptyStream => None,
        }
    }

    fn into_leg_error(self) -> LegError {
        match self {
            StartError::Genai(e) => LegError::Start(e.to_string()),
            StartError::FirstChunkTimeout => LegError::Start("first-chunk timeout".into()),
            StartError::EmptyStream => LegError::Start("empty stream".into()),
        }
    }
}

/// Open a genai stream for `leg` and await its first `StreamItem` within
/// `first_chunk`, wrapped in `provider.breaker` and retry policy.
async fn commit_one_leg(
    provider: &Arc<Provider>,
    leg: &ChainLeg,
    req: &ChatRequest,
    first_chunk: std::time::Duration,
) -> Result<CommittedStream, ResilienceError<StartError>> {
    let client = provider.client.clone();
    let provider_id = leg.provider.clone();
    let model = leg.model.clone();
    let chat_req = to_genai_request(req);
    let opts = to_genai_options(req, leg.effort)
        .with_capture_usage(true)
        .with_capture_tool_calls(true);

    run_with_classifier(
        move || {
            let (client, provider_id, model, chat_req, opts) = (
                client.clone(),
                provider_id.clone(),
                model.clone(),
                chat_req.clone(),
                opts.clone(),
            );
            async move {
                let resp = client
                    .exec_chat_stream(model.clone(), chat_req, Some(&opts))
                    .await
                    .map_err(StartError::Genai)?;
                let mut state = St {
                    inner: Box::pin(resp.stream),
                    indexer: ToolIndexer::default(),
                    input_tokens: 0,
                    output_tokens: 0,
                };
                match tokio::time::timeout(first_chunk, next_genai_item(&mut state)).await {
                    Err(_) => Err(StartError::FirstChunkTimeout),
                    Ok(None) => Err(StartError::EmptyStream),
                    Ok(Some(Err(e))) => Err(StartError::Genai(e)),
                    Ok(Some(Ok(first))) => {
                        let rest = futures::stream::once(async move { Ok(first) })
                            .chain(normalized_stream(state));
                        Ok(CommittedStream {
                            provider: provider_id,
                            model,
                            stream: rest.boxed(),
                        })
                    }
                }
            }
        },
        provider.profile,
        &provider.breaker,
        provider.label,
        StartError::is_retryable,
    )
    .await
}

enum BufferedLegError {
    CircuitOpen {
        name: String,
    },
    Failed {
        error: LegError,
        kind: &'static str,
        status: Option<u16>,
    },
}

/// Like `buffer_one_leg` but bounded by first-chunk and inter-chunk idle timeouts,
/// with per-leg circuit-breaker and retry on stream start.
async fn buffer_one_leg_timed(
    catalog: &Catalog,
    leg: &ChainLeg,
    req: &ChatRequest,
    t: StreamTimeouts,
) -> Result<Completion, BufferedLegError> {
    let provider = catalog
        .get(&leg.provider)
        .ok_or_else(|| BufferedLegError::Failed {
            error: LegError::Start(format!("unbuilt provider '{}'", leg.provider)),
            kind: "unbuilt_provider",
            status: None,
        })?;
    let committed = match commit_one_leg(provider, leg, req, t.first_chunk).await {
        Ok(c) => c,
        Err(ResilienceError::CircuitOpen { name }) => {
            return Err(BufferedLegError::CircuitOpen { name });
        }
        Err(ResilienceError::Exhausted(se)) => {
            let kind = se.kind();
            let status = se.status();
            return Err(BufferedLegError::Failed {
                error: se.into_leg_error(),
                kind,
                status,
            });
        }
    };
    let mut stream = committed.stream;
    let mut acc = Accumulator::default();
    loop {
        match tokio::time::timeout(t.idle, stream.next()).await {
            Err(_) => {
                let error = LegError::MidStream("idle timeout".into());
                provider.breaker.record::<(), &LegError>(&Err(&error));
                return Err(BufferedLegError::Failed {
                    error,
                    kind: "idle_timeout",
                    status: None,
                });
            }
            Ok(None) => break,
            Ok(Some(Ok(item))) => acc.push(item),
            Ok(Some(Err(error))) => {
                provider.breaker.record::<(), &LegError>(&Err(&error));
                return Err(BufferedLegError::Failed {
                    error,
                    kind: "mid_stream",
                    status: None,
                });
            }
        }
    }
    if !acc.got_done {
        let error = LegError::MidStream("stream ended before completion".into());
        provider.breaker.record::<(), &LegError>(&Err(&error));
        return Err(BufferedLegError::Failed {
            error,
            kind: "incomplete_stream",
            status: None,
        });
    }
    Ok(Completion {
        provider: leg.provider.clone(),
        model: leg.model.clone(),
        content: acc.content,
        tool_calls: acc.tool_calls,
        finish_reason: acc.finish_reason,
        input_tokens: acc.input_tokens,
        output_tokens: acc.output_tokens,
    })
}

/// Buffered executor with explicit first-chunk and idle timeouts. Walk legs,
/// fully buffering each. Any failure (start, timeout, or mid-stream) advances
/// to the next leg. Nothing is ever flushed to the client, so full-chain
/// fallback is preserved.
pub async fn execute_buffered_with_timeouts(
    catalog: &Catalog,
    route_name: &str,
    legs: &[ChainLeg],
    req: &ChatRequest,
    t: StreamTimeouts,
) -> Result<Completion, GatewayError> {
    let mut failures: Vec<LegFailure> = Vec::new();
    let mut all_circuit_open = true;
    for (i, leg) in legs.iter().enumerate() {
        match buffer_one_leg_timed(catalog, leg, req, t).await {
            Ok(c) => return Ok(c),
            Err(BufferedLegError::CircuitOpen { name }) => {
                if let Some(next) = legs.get(i + 1) {
                    log_leg_fallback(route_name, leg, next, "circuit_open", None);
                }
                failures.push(LegFailure {
                    provider: leg.provider.clone(),
                    model: leg.model.clone(),
                    message: format!("circuit open: {name}"),
                });
            }
            Err(BufferedLegError::Failed {
                error,
                kind,
                status,
            }) => {
                all_circuit_open = false;
                if let Some(next) = legs.get(i + 1) {
                    log_leg_fallback(route_name, leg, next, kind, status);
                }
                failures.push(LegFailure {
                    provider: leg.provider.clone(),
                    model: leg.model.clone(),
                    message: error.to_string(),
                });
            }
        }
    }
    if all_circuit_open && !failures.is_empty() {
        return Err(GatewayError::AllCircuitsOpen(route_name.to_string()));
    }
    Err(GatewayError::AllLegsFailed {
        route: route_name.to_string(),
        failures,
    })
}

/// Buffered (non-streaming) executor: walk legs, fully buffering each. Any
/// failure (start or mid-stream) advances to the next leg. Nothing is ever
/// flushed to the client, so full-chain fallback is preserved.
///
/// Delegates to [`execute_buffered_with_timeouts`] with [`StreamTimeouts::default`].
pub async fn execute_buffered(
    catalog: &Catalog,
    route_name: &str,
    legs: &[ChainLeg],
    req: &ChatRequest,
) -> Result<Completion, GatewayError> {
    execute_buffered_with_timeouts(catalog, route_name, legs, req, StreamTimeouts::default()).await
}

/// A committed streaming leg: the winning provider/model plus the remaining
/// item stream (first item already re-prepended).
pub struct CommittedStream {
    pub provider: String,
    pub model: String,
    pub stream: BoxStream<'static, Result<StreamItem, LegError>>,
}

impl CommittedStream {
    /// Wrap an already-obtained stream as a single committed leg (used by the
    /// native Vertex lane, which has no standard-lane fallback).
    pub fn single(
        provider: String,
        model: String,
        stream: impl Stream<Item = Result<StreamItem, LegError>> + Send + 'static,
    ) -> Self {
        Self {
            provider,
            model,
            stream: stream.boxed(),
        }
    }
}

/// Streaming executor with explicit first-chunk timeout. For each leg, start
/// the stream and peek the first item within `t.first_chunk`. First leg to
/// yield an item is committed; failures (including timeout) before the first
/// item fall back. After commitment there is no fallback.
pub async fn execute_streaming_with_timeouts(
    catalog: &Catalog,
    route_name: &str,
    legs: &[ChainLeg],
    req: &ChatRequest,
    t: StreamTimeouts,
) -> Result<CommittedStream, GatewayError> {
    let mut failures: Vec<LegFailure> = Vec::new();
    let mut all_circuit_open = true;

    for (i, leg) in legs.iter().enumerate() {
        let provider = match catalog.get(&leg.provider) {
            Some(p) => p,
            None => {
                all_circuit_open = false;
                if let Some(next) = legs.get(i + 1) {
                    log_leg_fallback(route_name, leg, next, "unbuilt_provider", None);
                }
                failures.push(LegFailure {
                    provider: leg.provider.clone(),
                    model: leg.model.clone(),
                    message: format!("unbuilt provider '{}'", leg.provider),
                });
                continue;
            }
        };
        match commit_one_leg(provider, leg, req, t.first_chunk).await {
            Ok(committed) => return Ok(committed),
            Err(ResilienceError::CircuitOpen { name }) => {
                if let Some(next) = legs.get(i + 1) {
                    log_leg_fallback(route_name, leg, next, "circuit_open", None);
                }
                failures.push(LegFailure {
                    provider: leg.provider.clone(),
                    model: leg.model.clone(),
                    message: format!("circuit open: {name}"),
                });
            }
            Err(ResilienceError::Exhausted(se)) => {
                all_circuit_open = false;
                let kind = se.kind();
                let status = se.status();
                if let Some(next) = legs.get(i + 1) {
                    log_leg_fallback(route_name, leg, next, kind, status);
                }
                failures.push(LegFailure {
                    provider: leg.provider.clone(),
                    model: leg.model.clone(),
                    message: se.into_leg_error().to_string(),
                });
            }
        }
    }
    if all_circuit_open && !failures.is_empty() {
        return Err(GatewayError::AllCircuitsOpen(route_name.to_string()));
    }
    Err(GatewayError::AllLegsFailed {
        route: route_name.to_string(),
        failures,
    })
}

/// Streaming executor: for each leg, start the stream and peek the first item.
/// First leg to yield an item is committed; failures before the first item fall
/// back. After commitment there is no fallback (caller surfaces mid-stream
/// errors as SSE error events).
///
/// Delegates to [`execute_streaming_with_timeouts`] with [`StreamTimeouts::default`].
pub async fn execute_streaming(
    catalog: &Catalog,
    route_name: &str,
    legs: &[ChainLeg],
    req: &ChatRequest,
) -> Result<CommittedStream, GatewayError> {
    execute_streaming_with_timeouts(catalog, route_name, legs, req, StreamTimeouts::default()).await
}
