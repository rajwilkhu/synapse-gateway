//! Name→scanner registry. Turns a `ScannerSpec` into one or more
//! `llm-guard` scanners. A bundle alias (e.g. `prompt_injection`) yields
//! several. There is no global registry in `llm-guard`, so this is ours.

use anyhow::{bail, Context};
use llm_guard::patterns::COMMON_INJECTION_PATTERNS;
use llm_guard::scanners::{
    BanSubstrings, InvisibleText, PiiPatterns, RegexPattern, RegexScan, RoleOverride, ScriptMix,
    TokenLimit,
};
use llm_guard::{Confidence, Match, ScanResult, Scanner, Severity};

use super::policy::{DetailedScanner, ScannerSpec};

/// Adapter so a `Box<dyn Scanner>` can be handed to `Pipeline::with`,
/// which takes `impl Scanner + 'static`. `Box<dyn Scanner>` does not itself
/// implement `Scanner`, so we delegate through this newtype.
pub struct BoxedScanner(pub Box<dyn Scanner>);

impl Scanner for BoxedScanner {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn scan<'a>(&self, input: &'a str) -> ScanResult<'a> {
        self.0.scan(input)
    }
}

fn parse_severity(s: &str) -> anyhow::Result<Severity> {
    match s.to_ascii_lowercase().as_str() {
        "block" => Ok(Severity::Block),
        "warn" => Ok(Severity::Warn),
        "info" => Ok(Severity::Info),
        other => bail!("unknown severity '{other}' (expected block|warn|info)"),
    }
}

/// Promote owned config substrings to `'static`. Called once per
/// `ban_substrings` policy at startup; the pipelines live for the whole
/// process, so this bounded leak is intentional.
fn leak_substrings(subs: &[String]) -> &'static [&'static str] {
    let leaked: Vec<&'static str> = subs
        .iter()
        .map(|s| &*Box::leak(s.clone().into_boxed_str()))
        .collect();
    Box::leak(leaked.into_boxed_slice())
}

/// Build the scanner(s) for one spec. Unknown names / missing params error.
pub fn build_scanners(spec: &ScannerSpec) -> anyhow::Result<Vec<Box<dyn Scanner>>> {
    match spec {
        ScannerSpec::Named(name) => build(name, None),
        ScannerSpec::Detailed(d) => build(&d.kind, Some(d)),
    }
}

fn block_pattern(id: &'static str, pat: &str) -> anyhow::Result<RegexPattern> {
    Ok(RegexPattern::new(id, pat)?
        .with_severity(Severity::Block)
        .with_confidence(Confidence::High))
}

/// Deterministic isolation-bypass signals: forging tenant identity headers,
/// naming internal cluster hosts, or curling/wgetting those hosts directly.
/// A header counts as set when its name is followed by `:`/`=` (curl `-H`,
/// dict/object literals), by `] =` (subscript assignment), or is the first
/// argument of a setter call (`set`, `append`, `header`, ...). Header reads
/// and comma-separated name lists do not match.
fn isolation_bypass_scanner() -> anyhow::Result<RegexScan> {
    let patterns = vec![
        block_pattern(
            "forged_identity_header",
            r#"(?i)(?:^|[^a-z0-9-])x-(?:tenant|workspace|user|agent)-id(?:["']?\s*[:=]|["']\s*\]\s*=)|\b(?:set|append|add|put|insert|header|setheader|set_header|add_header)\s*\(\s*["']x-(?:tenant|workspace|user|agent)-id["']\s*,"#,
        )?,
        block_pattern(
            "internal_host",
            r"(?i)[a-z0-9.-]+\.svc\.cluster(?:set)?\.local",
        )?,
        block_pattern(
            "direct_internal_call",
            r"(?i)\b(?:curl|wget)\b[^\n]*\.svc\.cluster(?:set)?\.local",
        )?,
    ];
    Ok(RegexScan::new("isolation_bypass", patterns))
}

/// Deterministic unbounded-view signals: a recursive flag in content that
/// calls `/fs/ls`, or a cortex URI scoped at a bare tenant/workspace root.
fn data_exfil_scanner() -> anyhow::Result<RegexScan> {
    let patterns = vec![
        block_pattern(
            "recursive_listing",
            r#"(?is)/fs/ls\b.*?recursive["'\s]*[:=]\s*["']?true\b|recursive["'\s]*[:=]\s*["']?true\b.*?/fs/ls\b"#,
        )?,
        block_pattern(
            "root_scope",
            r#"(?m)cortex://[^/\s"']+/[^/\s"']+/(?:[^A-Za-z0-9._~%-]|$)"#,
        )?,
    ];
    Ok(RegexScan::new("data_exfil", patterns))
}

fn secret_pattern(
    id: &'static str,
    pat: &str,
    confidence: Confidence,
) -> anyhow::Result<RegexPattern> {
    Ok(RegexPattern::new(id, pat)?
        .with_severity(Severity::Block)
        .with_confidence(confidence))
}

/// Credential patterns. `llm-guard`'s `Secrets` patterns, except that an AWS
/// key ID counts only outside a presigned URL and a labelled AWS secret
/// access key is added.
struct SecretsScan(RegexScan);

const PRESIGNED_KEY_ID_PREFIX: &str = "x-amz-credential=";

impl SecretsScan {
    fn new() -> anyhow::Result<Self> {
        let patterns = vec![
            secret_pattern(
                "openai_key",
                r"\bsk-(?:proj-|ant-)?[A-Za-z0-9_-]{20,}\b",
                Confidence::High,
            )?,
            secret_pattern(
                "aws_access_key",
                r"\b(?:AKIA|ASIA|AGPA|AIDA|AROA|AIPA|ANPA|ANVA)[A-Z0-9]{16}\b",
                Confidence::High,
            )?,
            secret_pattern(
                "aws_secret_key",
                r#"(?i:aws_?secret_?(?:access_?)?key|secret_?access_?key)["']?\s*[:=]\s*["']?[A-Za-z0-9/+]{40}"#,
                Confidence::High,
            )?,
            secret_pattern(
                "github_token",
                r"\bgh[psoru]_[A-Za-z0-9]{36,}\b",
                Confidence::High,
            )?,
            secret_pattern(
                "slack_token",
                r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b",
                Confidence::High,
            )?,
            secret_pattern(
                "stripe_key",
                r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}\b",
                Confidence::High,
            )?,
            secret_pattern(
                "pem_private_key",
                r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY-----",
                Confidence::High,
            )?,
            secret_pattern(
                "jwt",
                r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
                Confidence::Medium,
            )?,
        ];
        Ok(Self(RegexScan::new("secrets", patterns)))
    }
}

/// A presigned URL carries the key ID, never the secret key; it grants one
/// request on one object until it expires.
fn in_presigned_url(input: &str, m: &Match<'_>) -> bool {
    let before = &input.as_bytes()[..m.span.start];
    m.pattern == "aws_access_key"
        && before.len() >= PRESIGNED_KEY_ID_PREFIX.len()
        && before[before.len() - PRESIGNED_KEY_ID_PREFIX.len()..]
            .eq_ignore_ascii_case(PRESIGNED_KEY_ID_PREFIX.as_bytes())
}

/// The regex stops at 40 characters; a longer run is some other value.
fn longer_than_a_secret_key(input: &str, m: &Match<'_>) -> bool {
    m.pattern == "aws_secret_key"
        && input[m.span.end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '/' || c == '+')
}

impl Scanner for SecretsScan {
    fn name(&self) -> &'static str {
        "secrets"
    }

    fn scan<'a>(&self, input: &'a str) -> ScanResult<'a> {
        let mut result = self.0.scan(input);
        result
            .matches
            .retain(|m| !in_presigned_url(input, m) && !longer_than_a_secret_key(input, m));
        result
    }
}

fn build(name: &str, d: Option<&DetailedScanner>) -> anyhow::Result<Vec<Box<dyn Scanner>>> {
    let scanners: Vec<Box<dyn Scanner>> = match name {
        "secrets" => vec![Box::new(SecretsScan::new()?)],
        "pii" => vec![Box::new(PiiPatterns::new())],
        "isolation_bypass" => vec![Box::new(isolation_bypass_scanner()?)],
        "data_exfil" => vec![Box::new(data_exfil_scanner()?)],
        "invisible_text" => vec![Box::new(InvisibleText::new())],
        "role_override" => vec![Box::new(RoleOverride::new())],
        "script_mix" => {
            let threshold = d.and_then(|d| d.threshold).unwrap_or(2);
            vec![Box::new(ScriptMix::new(threshold))]
        }
        "token_limit" => {
            let max = d
                .and_then(|d| d.max_chars)
                .context("scanner 'token_limit' requires 'max_chars'")?;
            vec![Box::new(TokenLimit::new(max))]
        }
        "ban_substrings" => {
            let d = d.context("scanner 'ban_substrings' requires a table with 'substrings'")?;
            if d.substrings.is_empty() {
                bail!("scanner 'ban_substrings' requires a non-empty 'substrings' list");
            }
            let severity = match &d.severity {
                Some(s) => parse_severity(s)?,
                None => Severity::Block,
            };
            let patterns = leak_substrings(&d.substrings);
            vec![Box::new(
                BanSubstrings::new("ban_substrings", patterns).with_severity(severity),
            )]
        }
        "prompt_injection" => vec![
            Box::new(
                BanSubstrings::new("injection", COMMON_INJECTION_PATTERNS)
                    .with_severity(Severity::Block),
            ),
            Box::new(RoleOverride::new()),
        ],
        other => bail!("unknown scanner '{other}'"),
    };
    Ok(scanners)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::policy::GuardrailsConfig;

    fn spec(toml: &str) -> ScannerSpec {
        let cfg = GuardrailsConfig::from_toml_str(toml).unwrap();
        cfg.guardrails.get("p").unwrap().scanners[0].clone()
    }

    #[test]
    fn builds_known_named_scanner() {
        let s = spec(
            r#"[guardrails.p]
                        scanners = ["secrets"]"#,
        );
        let built = build_scanners(&s).unwrap();
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].name(), "secrets");
    }

    #[test]
    fn prompt_injection_expands_to_bundle() {
        let s = spec(
            r#"[guardrails.p]
                        scanners = ["prompt_injection"]"#,
        );
        assert_eq!(build_scanners(&s).unwrap().len(), 2);
    }

    #[test]
    fn ban_substrings_blocks_by_default_and_is_case_insensitive() {
        let s = spec(
            r#"[guardrails.p]
                        scanners = [{ type = "ban_substrings", substrings = ["forbidden"] }]"#,
        );
        let built = build_scanners(&s).unwrap();
        let r = built[0].scan("This is FORBIDDEN text");
        assert!(r.should_refuse(), "default severity must be Block");
    }

    #[test]
    fn token_limit_requires_max_chars() {
        let s = spec(
            r#"[guardrails.p]
                        scanners = [{ type = "token_limit" }]"#,
        );
        let err = build_scanners(&s).err().expect("expected an error");
        assert!(err.to_string().contains("max_chars"));
    }

    #[test]
    fn unknown_scanner_errors() {
        let s = spec(
            r#"[guardrails.p]
                        scanners = ["nope"]"#,
        );
        let err = build_scanners(&s).err().expect("expected an error");
        assert!(err.to_string().contains("unknown scanner"));
    }

    #[test]
    fn isolation_bypass_flags_forged_header_and_internal_host() {
        let built = build("isolation_bypass", None).unwrap();
        assert_eq!(built.len(), 1);
        let s = &built[0];
        assert_eq!(s.name(), "isolation_bypass");
        assert!(s
            .scan(r#"curl -H "X-Tenant-Id: org-123" https://x"#)
            .should_refuse());
        assert!(s
            .scan("http://cortex-server.infrastructure.svc.clusterset.local/health")
            .should_refuse());
        assert!(s
            .scan(r#"requests.get("http://cortex.infra.SVC.Cluster.Local/x")"#)
            .should_refuse());
    }

    #[test]
    fn isolation_bypass_flags_header_set_from_code() {
        let s = &build("isolation_bypass", None).unwrap()[0];
        assert!(s.scan(r#"headers={"X-Tenant-Id": org}"#).should_refuse());
        assert!(s
            .scan(r#"fetch(u, {headers: {'X-Workspace-Id': ws}})"#)
            .should_refuse());
    }

    #[test]
    fn isolation_bypass_flags_header_setter_calls() {
        let s = &build("isolation_bypass", None).unwrap()[0];
        assert!(s.scan(r#"headers["X-Tenant-Id"] = org"#).should_refuse());
        assert!(s
            .scan(r#"req.headers.set("X-Tenant-Id", v)"#)
            .should_refuse());
        assert!(s.scan(r#"h.Set('X-User-Id', v)"#).should_refuse());
        assert!(s.scan(r#".header("X-Agent-Id", id)"#).should_refuse());
    }

    #[test]
    fn isolation_bypass_ignores_header_name_list_in_prose() {
        let s = &build("isolation_bypass", None).unwrap()[0];
        assert!(!s
            .scan("Supported headers: X-Tenant-Id, X-Workspace-Id and more.")
            .flagged());
        assert!(!s
            .scan("allowed headers: 'X-Tenant-Id', 'X-Workspace-Id'")
            .flagged());
    }

    #[test]
    fn isolation_bypass_ignores_longer_names_and_header_reads() {
        let s = &build("isolation_bypass", None).unwrap()[0];
        assert!(!s.scan("max-user-id=5").flagged());
        assert!(!s.scan("inbox-user-id: 3").flagged());
        assert!(!s.scan(r#"headers.get("X-Tenant-Id", fallback)"#).flagged());
    }

    #[test]
    fn isolation_bypass_ignores_plain_prose() {
        let s = &build("isolation_bypass", None).unwrap()[0];
        assert!(!s
            .scan("This skill summarises the tenant onboarding flow.")
            .flagged());
    }

    // A literal header token in prose also matches.
    #[test]
    fn isolation_bypass_flags_header_token_even_in_prose() {
        let s = &build("isolation_bypass", None).unwrap()[0];
        assert!(s
            .scan("Set the X-Tenant-Id: header before calling.")
            .should_refuse());
    }

    #[test]
    fn data_exfil_flags_recursive_listing_and_root_scope() {
        let s = &build("data_exfil", None).unwrap()[0];
        assert_eq!(s.name(), "data_exfil");
        assert!(s
            .scan("curl -G \"$CX/api/v1/fs/ls\" \\\n  --data-urlencode \"recursive=true\"")
            .should_refuse());
        assert!(s
            .scan(r#"requests.get(f"{cx}/api/v1/fs/ls", params={"recursive": "true"})"#)
            .should_refuse());
        assert!(s.scan(r#"uri=cortex://$ORG/$WS/""#).should_refuse());
        for root in [
            "fetch(cortex://org/ws/)",
            "cortex://org/ws/, then",
            "see\ncortex://org/ws/\nnext",
        ] {
            assert!(s.scan(root).should_refuse(), "{root}");
        }
    }

    #[test]
    fn data_exfil_ignores_recursive_flag_without_fs_ls() {
        let s = &build("data_exfil", None).unwrap()[0];
        assert!(!s.scan(r#"glob.glob("**/*.md", recursive=True)"#).flagged());
        assert!(!s.scan("fs.mkdirSync(p, {recursive: true})").flagged());
    }

    #[test]
    fn data_exfil_ignores_bounded_deep_path_read() {
        let s = &build("data_exfil", None).unwrap()[0];
        assert!(!s
            .scan(r#"uri=cortex://org/ws/agent/a1/skills/run.sh"#)
            .flagged());
    }

    /// AWS's documented example credentials.
    const KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";
    const SECRET_KEY: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    fn patterns(text: &str) -> Vec<&'static str> {
        let s = &build("secrets", None).unwrap()[0];
        s.scan(text).matches.iter().map(|m| m.pattern).collect()
    }

    #[test]
    fn secrets_ignores_the_key_id_of_a_presigned_url() {
        let url = format!(
            "[file.pdf](https://prod-files-secure.s3.us-west-2.amazonaws.com/a/b/file.pdf\
             ?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Content-Sha256=UNSIGNED-PAYLOAD\
             &X-Amz-Credential={KEY_ID}%2F20261023%2Fus-west-2%2Fs3%2Faws4_request\
             &X-Amz-Date=20261023T120000Z&X-Amz-Expires=3600&X-Amz-Signature=abc123\
             &X-Amz-SignedHeaders=host&x-id=GetObject)"
        );
        assert_eq!(patterns(&url), Vec::<&str>::new());
        assert_eq!(
            patterns(&format!("x-amz-credential={KEY_ID}/20261023")),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn secrets_flags_a_bare_key_id() {
        assert_eq!(
            patterns(&format!("the key is {KEY_ID}")),
            ["aws_access_key"]
        );
    }

    #[test]
    fn secrets_flags_a_labelled_secret_access_key() {
        for text in [
            format!("aws_secret_access_key = {SECRET_KEY}"),
            format!("AWS_SECRET_ACCESS_KEY={SECRET_KEY}"),
            format!(r#"{{"SecretAccessKey": "{SECRET_KEY}"}}"#),
        ] {
            assert_eq!(patterns(&text), ["aws_secret_key"], "{text}");
        }
    }

    #[test]
    fn secrets_ignores_unlabelled_or_longer_40_char_strings() {
        assert_eq!(
            patterns(&format!("checksum {SECRET_KEY}")),
            Vec::<&str>::new()
        );
        assert_eq!(
            patterns(&format!("aws_secret_access_key = {SECRET_KEY}abc")),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn secrets_keeps_the_other_credential_patterns() {
        assert_eq!(
            patterns("token: ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ["github_token"]
        );
    }
}
