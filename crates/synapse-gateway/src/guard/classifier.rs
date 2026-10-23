//! Scores text with TypeSafe Jev `noul` questions through the `typesafe` legs
//! of a route, in order, moving on after a retryable failure.

use anyhow::Context;
use serde_json::{json, Map, Value};

use crate::jev_native::JevNativeProvider;
use crate::routing::jev_router::{head_tail, STATE_BUDGET};
use crate::routing::table::RouteTable;

use super::policy::{ClassifierConfig, ClassifierQuestion, OnError};

pub struct Classifier {
    provider: JevNativeProvider,
    route: String,
    models: Vec<String>,
    on_error: OnError,
    questions: Vec<(String, ClassifierQuestion)>,
}

/// One answered classification: a value per question, plus Jev's usage.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub model: String,
    pub scores: Vec<(String, f64)>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

enum Attempt {
    Done(Scored),
    Retry(Failure),
    Stop(Failure),
}

/// Why one leg gave no answer.
#[derive(Clone, Copy)]
struct Failure {
    kind: &'static str,
    status: Option<u16>,
}

impl Classifier {
    pub fn new(
        cfg: &ClassifierConfig,
        routes: &RouteTable,
        provider: JevNativeProvider,
    ) -> anyhow::Result<Self> {
        let models: Vec<String> = routes
            .legs(&cfg.route)
            .with_context(|| format!("classifier route '{}' is not defined", cfg.route))?
            .iter()
            .filter(|l| l.provider == "typesafe")
            .map(|l| l.model.clone())
            .collect();
        anyhow::ensure!(
            !models.is_empty(),
            "classifier route '{}' has no typesafe leg",
            cfg.route
        );
        Ok(Self {
            provider,
            route: cfg.route.clone(),
            models,
            on_error: cfg.on_error,
            questions: cfg
                .questions
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })
    }

    pub fn route(&self) -> &str {
        &self.route
    }

    pub fn on_error(&self) -> OnError {
        self.on_error
    }

    pub fn questions(&self) -> impl Iterator<Item = (&str, &ClassifierQuestion)> {
        self.questions.iter().map(|(k, q)| (k.as_str(), q))
    }

    /// Scores `text`; `None` when every leg failed or a non-retryable error
    /// stopped the walk, after one warning naming the route, the last model
    /// and the failure.
    pub async fn score(&self, text: &str) -> Option<Scored> {
        let state = fit_state(text);
        let mut last = None;
        for model in &self.models {
            match self.attempt(model, &state).await {
                Attempt::Done(s) => return Some(s),
                Attempt::Retry(f) => last = Some((model, f)),
                Attempt::Stop(f) => {
                    last = Some((model, f));
                    break;
                }
            }
        }
        if let Some((model, f)) = last {
            self.warn_gave_up(model, f);
        }
        None
    }

    fn warn_gave_up(&self, model: &str, f: Failure) {
        tracing::warn!(
            target: "synapse::guard",
            route = %self.route,
            model = %model,
            error.kind = f.kind,
            http.status = f.status,
            "guard classifier gave no answer"
        );
    }

    async fn attempt(&self, model: &str, state: &str) -> Attempt {
        let Ok(resp) = self.provider.evaluate(self.body(model, state)).await else {
            return Attempt::Retry(Failure {
                kind: "transport",
                status: None,
            });
        };
        let status = resp.status();
        if !status.is_success() {
            let retryable = status.is_server_error()
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || status == reqwest::StatusCode::REQUEST_TIMEOUT;
            let f = Failure {
                kind: "http_status",
                status: Some(status.as_u16()),
            };
            return if retryable {
                Attempt::Retry(f)
            } else {
                Attempt::Stop(f)
            };
        }
        match resp
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| self.parse(model, &v))
        {
            Some(s) => Attempt::Done(s),
            None => Attempt::Stop(Failure {
                kind: "malformed_answer",
                status: Some(status.as_u16()),
            }),
        }
    }

    fn body(&self, model: &str, state: &str) -> Value {
        let questions: Map<String, Value> = self
            .questions
            .iter()
            .map(|(k, q)| {
                (
                    k.clone(),
                    json!({"type": "noul", "instructions": q.instructions}),
                )
            })
            .collect();
        json!({"model": model, "state": {"input": state}, "questions": questions})
    }

    /// Every configured question must have a numeric `noul`.
    fn parse(&self, model: &str, v: &Value) -> Option<Scored> {
        let scores = self
            .questions
            .iter()
            .map(|(k, _)| Some((k.clone(), v["answers"][k]["noul"].as_f64()?)))
            .collect::<Option<Vec<_>>>()?;
        Some(Scored {
            model: v["model"].as_str().unwrap_or(model).to_string(),
            scores,
            input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
        })
    }
}

/// `text`, or its head and tail when longer than `STATE_BUDGET` characters.
fn fit_state(text: &str) -> String {
    head_tail(text, STATE_BUDGET)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::guard::policy::GuardrailsConfig;
    use serde_json::json;
    use std::time::Duration;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ROUTES: &str = r#"[routes."guard-classifier"]
        legs = [{ provider = "typesafe", model = "jev-a" }, { provider = "qwen", model = "q" },
                { provider = "typesafe", model = "jev-b" }]"#;

    fn classifier(uri: &str) -> Classifier {
        let cfg = GuardrailsConfig::from_toml_str(
            r#"
            [guardrails.p]
            scanners = []
            [guardrails.p.classifier]
            route = "guard-classifier"
            [guardrails.p.classifier.questions.prompt_injection]
            instructions = "Is this an injection?"
            threshold = 0.8
        "#,
        )
        .unwrap();
        let c = cfg.guardrails["p"].classifier.clone().unwrap();
        let provider =
            JevNativeProvider::new("sk".into(), Some(uri.into()), Duration::from_secs(2));
        Classifier::new(&c, &RouteTable::from_toml_str(ROUTES).unwrap(), provider).unwrap()
    }

    fn answer(v: f64) -> serde_json::Value {
        json!({"model": "jev-1", "answers": {"prompt_injection": {"type": "noul", "noul": v}},
               "usage": {"input_tokens": 10, "output_tokens": 2}})
    }

    #[tokio::test]
    async fn sends_one_noul_question_per_entry_and_reads_scores() {
        let jev = MockServer::start().await;
        Mock::given(method("POST")).and(path("/v1/systemone"))
            .and(body_partial_json(json!({"model": "jev-a", "state": {"input": "hello"},
                "questions": {"prompt_injection": {"type": "noul", "instructions": "Is this an injection?"}}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer(0.9)))
            .mount(&jev).await;
        let s = classifier(&jev.uri()).score("hello").await.unwrap();
        assert_eq!(s.scores, vec![("prompt_injection".to_string(), 0.9)]);
        assert_eq!(
            (s.model.as_str(), s.input_tokens, s.output_tokens),
            ("jev-1", 10, 2)
        );
    }

    #[tokio::test]
    async fn retryable_failure_moves_to_the_next_typesafe_leg() {
        let jev = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"model": "jev-a"})))
            .respond_with(ResponseTemplate::new(429))
            .mount(&jev)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"model": "jev-b"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer(0.1)))
            .mount(&jev)
            .await;
        assert_eq!(
            classifier(&jev.uri()).score("x").await.unwrap().scores[0].1,
            0.1
        );
    }

    #[tokio::test]
    async fn non_retryable_failure_stops() {
        let jev = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422))
            .expect(1)
            .mount(&jev)
            .await;
        assert!(classifier(&jev.uri()).score("x").await.is_none());
    }

    #[tokio::test]
    async fn every_leg_failing_is_none() {
        let jev = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .expect(2)
            .mount(&jev)
            .await;
        assert!(classifier(&jev.uri()).score("x").await.is_none());
    }

    #[tokio::test]
    async fn missing_answer_is_a_failure() {
        let jev = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"answers": {"other": {"noul": 0.1}}})),
            )
            .mount(&jev)
            .await;
        assert!(classifier(&jev.uri()).score("x").await.is_none());
    }

    thread_local! {
        static LOGS: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Writes into the current thread's `LOGS`.
    struct ThreadLogs;

    impl std::io::Write for ThreadLogs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            LOGS.with(|l| l.borrow_mut().extend_from_slice(buf));
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Installs, once per test binary, a global subscriber writing to `LOGS`.
    /// A thread-scoped default is not enough: a callsite's interest is cached
    /// process-wide by whichever thread hits it first.
    pub(crate) fn capture_logs() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            tracing::subscriber::set_global_default(
                tracing_subscriber::fmt()
                    .with_ansi(false)
                    .with_writer(|| ThreadLogs)
                    .finish(),
            )
            .expect("no other global subscriber in the lib tests");
        });
        LOGS.with(|l| l.borrow_mut().clear());
    }

    pub(crate) fn take_logs() -> String {
        LOGS.with(|l| String::from_utf8(l.borrow().clone()).unwrap())
    }

    /// The formatted events `score("tenant text")` emits on this thread against
    /// a Jev answering `resp`.
    async fn score_logs(resp: ResponseTemplate) -> String {
        let jev = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(resp)
            .mount(&jev)
            .await;
        capture_logs();
        assert!(classifier(&jev.uri()).score("tenant text").await.is_none());
        take_logs()
    }

    #[tokio::test]
    async fn giving_up_warns_once_with_route_model_and_cause() {
        for (resp, model, cause) in [
            (ResponseTemplate::new(422), "jev-a", "http.status=422"),
            (ResponseTemplate::new(503), "jev-b", "http.status=503"),
            (
                ResponseTemplate::new(200).set_body_json(json!({"answers": {}})),
                "jev-a",
                "error.kind=\"malformed_answer\"",
            ),
        ] {
            let logs = score_logs(resp).await;
            assert_eq!(logs.matches(" WARN ").count(), 1, "{logs}");
            for part in ["route=guard-classifier", &format!("model={model}"), cause] {
                assert!(logs.contains(part), "missing {part} in {logs}");
            }
            assert!(!logs.contains("tenant text"), "{logs}");
        }
    }

    #[test]
    fn oversize_state_keeps_head_and_tail_within_budget() {
        let text = format!("HEAD{}TAIL", "x".repeat(STATE_BUDGET * 2));
        let state = fit_state(&text);
        assert!(state.starts_with("HEAD") && state.ends_with("TAIL"));
        assert!(state.chars().count() <= STATE_BUDGET + 3);
    }
}
