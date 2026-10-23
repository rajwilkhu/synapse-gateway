//! Holds back the tail of a streamed reply as raw text so output scanners see
//! matches that span deltas, and releases a prefix only after it has been
//! screened and masked.

use std::sync::Arc;
use std::time::Instant;

use super::GuardEngine;

#[derive(Debug, PartialEq)]
pub enum Release {
    Text(String),
    Blocked(Vec<String>),
}

pub struct OutputGuard {
    engine: Arc<GuardEngine>,
    policy: String,
    holdback: usize,
    held: String,
    worst: &'static str,
    matched: Vec<String>,
    scan_secs: f64,
    recorded: bool,
}

impl OutputGuard {
    pub fn new(engine: Arc<GuardEngine>, policy: String) -> Self {
        Self {
            holdback: engine.holdback(&policy),
            engine,
            policy,
            held: String::new(),
            worst: "pass",
            matched: Vec::new(),
            scan_secs: 0.0,
            recorded: false,
        }
    }

    /// Screen `delta` with the held text and release all but the last
    /// `holdback` characters, screened and masked. A match crossing that
    /// release point is held from its start. Released text is never scanned
    /// again.
    pub fn push(&mut self, delta: &str) -> Release {
        self.held.push_str(delta);
        let chars = self.held.chars().count();
        let window_cut = match chars.checked_sub(self.holdback) {
            Some(n) => self
                .held
                .char_indices()
                .nth(n)
                .map_or(self.held.len(), |(i, _)| i),
            None => 0,
        };
        self.release(window_cut)
    }

    /// Release everything held, screened and masked.
    pub fn flush(&mut self) -> Release {
        self.release(self.held.len())
    }

    /// Release everything held and record the scan for the response.
    pub fn finish(&mut self) -> Release {
        let release = self.flush();
        self.record();
        release
    }

    fn release(&mut self, window_cut: usize) -> Release {
        let started = Instant::now();
        let (scan, released) = self
            .engine
            .release_output(&self.policy, &mut self.held, window_cut);
        self.scan_secs += started.elapsed().as_secs_f64();
        self.worst = worse(self.worst, scan.outcome);
        self.matched.extend(scan.matched);
        if scan.blockers.is_empty() {
            return Release::Text(released);
        }
        self.held.clear();
        self.record();
        Release::Blocked(scan.blockers)
    }

    fn record(&mut self) {
        if !self.recorded {
            self.recorded = true;
            self.matched.sort();
            self.matched.dedup();
            self.engine.record_output_scan_with_scanners(
                &self.policy,
                self.worst,
                self.scan_secs,
                &self.matched,
            );
        }
    }
}

impl Drop for OutputGuard {
    fn drop(&mut self) {
        self.record();
    }
}

fn rank(outcome: &str) -> u8 {
    match outcome {
        "block" => 4,
        "masked" => 3,
        "observe" => 2,
        "flag" => 1,
        _ => 0,
    }
}

fn worse(a: &'static str, b: &'static str) -> &'static str {
    if rank(b) > rank(a) {
        b
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::GuardrailsConfig;

    const SECRET: &str = "ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789";

    fn guard(holdback: usize) -> OutputGuard {
        let cfg = GuardrailsConfig::from_toml_str(&format!(
            r#"[guardrails.p]
            scanners = []
            output_scanners = [{{ type = "secrets", action = "mask" }},
                               {{ type = "ban_substrings", substrings = ["forbidden"] }}]
            output_holdback_chars = {holdback}"#
        ))
        .unwrap();
        OutputGuard::new(
            Arc::new(GuardEngine::from_config(&cfg).unwrap()),
            "p".into(),
        )
    }

    fn text(r: Release) -> String {
        match r {
            Release::Text(t) => t,
            Release::Blocked(s) => panic!("blocked by {s:?}"),
        }
    }

    #[test]
    fn a_secret_split_across_deltas_is_masked() {
        let mut g = guard(64);
        let (a, b) = SECRET.split_at(10);
        let (b, c) = b.split_at(10);
        let mut out = String::new();
        for d in ["key: ", a, b, c, " end"] {
            out += &text(g.push(d));
        }
        out += &text(g.flush());
        assert_eq!(out, "key: [REDACTED:secrets] end");
    }

    #[test]
    fn text_beyond_the_window_is_released_early() {
        let mut g = guard(4);
        assert_eq!(text(g.push("hello world")), "hello w");
        assert_eq!(text(g.flush()), "orld");
    }

    #[test]
    fn short_reply_is_released_at_done() {
        let mut g = guard(256);
        assert_eq!(text(g.push("hi")), "");
        assert_eq!(text(g.flush()), "hi");
    }

    #[test]
    fn window_counts_characters_not_bytes() {
        let mut g = guard(2);
        assert_eq!(text(g.push("héé")), "h");
    }

    #[test]
    fn block_returns_scanners_and_drops_held_text() {
        let mut g = guard(64);
        assert_eq!(text(g.push("this is ")), "");
        assert_eq!(
            g.push("forbidden"),
            Release::Blocked(vec!["ban_substrings".to_string()])
        );
        assert_eq!(text(g.flush()), "");
    }

    #[cfg(feature = "server")]
    #[test]
    fn drop_records_one_scan() {
        let (m, exporter) = crate::telemetry::test_metrics();
        let cfg = GuardrailsConfig::from_toml_str(
            r#"[guardrails.p]
            scanners = []
            output_scanners = ["secrets"]"#,
        )
        .unwrap();
        let engine = Arc::new(GuardEngine::from_config(&cfg).unwrap().with_metrics(m));
        {
            let mut g = OutputGuard::new(engine, "p".into());
            let _ = g.push("partial");
            let _ = g.push(" more");
        }
        let text = crate::telemetry::scrape(&exporter);
        assert!(text.lines().any(|l| l == r#"synapse_guard_scans_total{direction="output",lane="chat",outcome="pass",policy="p"} 1"#), "{text}");
    }

    #[test]
    fn secret_after_released_text_is_masked_whole() {
        let mut g = guard(48);
        let mut out = text(g.push(&"x ".repeat(100)));
        for chunk in SECRET.as_bytes().chunks(3) {
            out += &text(g.push(std::str::from_utf8(chunk).unwrap()));
        }
        out += &text(g.push(&" y".repeat(100)));
        out += &text(g.flush());
        assert_eq!(
            out,
            format!("{}[REDACTED:secrets]{}", "x ".repeat(100), " y".repeat(100))
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn a_match_is_recorded_once_across_pushes() {
        let (m, exporter) = crate::telemetry::test_metrics();
        let cfg = GuardrailsConfig::from_toml_str(
            r#"[guardrails.p]
            scanners = []
            output_scanners = [{ type = "secrets", action = "mask" }]
            output_holdback_chars = 48"#,
        )
        .unwrap();
        let engine = Arc::new(GuardEngine::from_config(&cfg).unwrap().with_metrics(m));
        let mut g = OutputGuard::new(engine, "p".into());
        let _ = g.push(&"x ".repeat(50));
        for chunk in SECRET.as_bytes().chunks(5) {
            let _ = g.push(std::str::from_utf8(chunk).unwrap());
        }
        let _ = g.push(&" y".repeat(50));
        let _ = g.flush();
        let text = crate::telemetry::scrape(&exporter);
        let n: Vec<_> = text
            .lines()
            .filter(|l| l.starts_with("synapse_guard_matches_total"))
            .collect();
        assert!(n.len() == 1 && n[0].ends_with(" 1"), "{text}");
    }

    const GH_LONG: &str = "ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789aBcDeFgH";

    fn stream(g: &mut OutputGuard, input: &str, step: usize) -> String {
        let chars: Vec<char> = input.chars().collect();
        let mut out = String::new();
        for c in chars.chunks(step) {
            out += &text(g.push(&c.iter().collect::<String>()));
        }
        out + &text(g.flush())
    }

    #[test]
    fn over_length_tokens_are_masked_whole() {
        for token in [GH_LONG.to_string(), format!("sk-proj-{}", "aB3".repeat(20))] {
            let mut g = guard(80);
            let out = stream(&mut g, &format!("key: {token} end"), 3);
            assert_eq!(out, "key: [REDACTED:secrets] end", "{token}");
        }
    }

    #[test]
    fn a_match_crossing_the_window_is_held_from_its_start() {
        let mut g = guard(4);
        let mut out = text(g.push("key: "));
        out += &text(g.push(&GH_LONG[..40]));
        for c in GH_LONG.as_bytes()[40..].chunks(2) {
            out += &text(g.push(std::str::from_utf8(c).unwrap()));
        }
        assert_eq!(out, "key: ");
        out += &text(g.flush());
        assert_eq!(out, "key: [REDACTED:secrets]");
    }

    #[test]
    fn a_match_ending_at_the_window_cut_is_released_masked() {
        let mut g = guard(1);
        assert_eq!(g.push("a "), Release::Text("a".into()));
        assert_eq!(
            g.push(&format!("{GH_LONG} ")),
            Release::Text(" [REDACTED:secrets]".into())
        );
        assert_eq!(g.flush(), Release::Text(" ".into()));
    }

    #[test]
    fn non_streaming_screen_masks_a_token_at_the_end() {
        let cfg = GuardrailsConfig::from_toml_str(
            r#"[guardrails.p]
            scanners = []
            output_scanners = [{ type = "secrets", action = "mask" }]"#,
        )
        .unwrap();
        let e = GuardEngine::from_config(&cfg).unwrap();
        let mut t = format!("key: {GH_LONG}");
        let scan = e.screen_output("p", &mut t);
        assert_eq!(
            (t.as_str(), scan.outcome),
            ("key: [REDACTED:secrets]", "masked")
        );
    }

    fn guard_with(scanners: &str, holdback: usize) -> OutputGuard {
        let cfg = GuardrailsConfig::from_toml_str(&format!(
            "[guardrails.p]\nscanners = []\noutput_scanners = [{scanners}]\noutput_holdback_chars = {holdback}"
        ))
        .unwrap();
        OutputGuard::new(
            Arc::new(GuardEngine::from_config(&cfg).unwrap()),
            "p".into(),
        )
    }

    fn stream_deltas<'a>(g: &mut OutputGuard, deltas: impl IntoIterator<Item = &'a str>) -> String {
        let mut out = String::new();
        for d in deltas {
            out += &text(g.push(d));
        }
        out + &text(g.flush())
    }

    // Split so secret scanners do not flag the fixture.
    const XOXB: &str = concat!(
        "xo",
        "xb-123456789012-1234567890123-AbCdEfGhIjKlMnOpQrStUvWx"
    );

    #[test]
    fn a_token_streamed_in_deltas_ending_at_hyphens_is_masked_whole() {
        for lead in [String::new(), "x ".repeat(100)] {
            let mut g = guard(64);
            let mut deltas = vec![lead.as_str(), "token "];
            deltas.extend(XOXB.split_inclusive('-'));
            deltas.push(" end");
            let out = stream_deltas(&mut g, deltas);
            assert_eq!(out, format!("{lead}token [REDACTED:secrets] end"));
        }
    }

    #[test]
    fn a_phone_number_streamed_per_token_is_masked_whole() {
        let mut g = guard_with(r#"{ type = "pii", action = "mask" }"#, 64);
        let mut deltas = vec!["call "];
        deltas.extend("+1-555-123-4567".split_inclusive('-'));
        deltas.push(" now");
        let out = stream_deltas(&mut g, deltas);
        assert_eq!(out, "call [REDACTED:pii_patterns] now");
    }

    #[test]
    fn a_mask_label_is_not_rescanned() {
        let mut g = guard_with(
            r#"{ type = "secrets", action = "mask" },
               { type = "ban_substrings", substrings = ["redacted"] }"#,
            48,
        );
        let input = format!("key: {SECRET} {}", "y ".repeat(40));
        let out = stream(&mut g, &input, 3);
        assert_eq!(out, format!("key: [REDACTED:secrets] {}", "y ".repeat(40)));
    }

    #[cfg(feature = "server")]
    #[test]
    fn a_growing_logged_match_is_recorded_once() {
        let (m, exporter) = crate::telemetry::test_metrics();
        let cfg = GuardrailsConfig::from_toml_str(
            r#"[guardrails.p]
            scanners = []
            output_scanners = [{ type = "secrets", action = "log" }]
            output_holdback_chars = 80"#,
        )
        .unwrap();
        let engine = Arc::new(GuardEngine::from_config(&cfg).unwrap().with_metrics(m));
        let mut g = OutputGuard::new(engine, "p".into());
        let out = stream(&mut g, &format!("key: {GH_LONG} end"), 3);
        assert_eq!(out, format!("key: {GH_LONG} end"));
        let text = crate::telemetry::scrape(&exporter);
        let n: Vec<_> = text
            .lines()
            .filter(|l| l.starts_with("synapse_guard_matches_total"))
            .collect();
        assert!(n.len() == 1 && n[0].ends_with(" 1"), "{text}");
    }

    #[test]
    fn output_guard_logs_non_pass_outcome_with_scanners() {
        crate::guard::classifier::tests::capture_logs();
        let _ = crate::guard::classifier::tests::take_logs();

        {
            let mut g = guard(48);
            let _ = stream(&mut g, &format!("key: {SECRET} end"), 5);
        }

        let logs = crate::guard::classifier::tests::take_logs();
        let expected =
            r#"policy="p" lane="chat" direction="output" outcome="masked" scanners=["secrets"]"#;
        assert!(
            logs.contains(expected),
            "expected `{expected}` in logs:\n{logs}"
        );
        assert!(
            !logs.contains(SECRET),
            "output guard log must never contain secret text:\n{logs}"
        );
    }
}
