//! Startup checks that the guardrails file and every policy a route names exist.

use crate::config::GuardrailsMode;
use crate::routing::embeddings::EmbeddingRouteTable;
use crate::routing::table::RouteTable;

use super::policy::{Action, ClassifierConfig, GuardrailsConfig, Policy, ScannerSpec};

/// Under `Required`, fails when the file is missing or a chat or embedding
/// route names a policy the file does not define. Under `Optional`, logs a
/// warning for each instead.
pub fn validate_guardrails(
    mode: GuardrailsMode,
    path: &str,
    guardrails: Option<&GuardrailsConfig>,
    routes: &RouteTable,
    embed_routes: &EmbeddingRouteTable,
) -> anyhow::Result<()> {
    let Some(cfg) = guardrails else {
        return match mode {
            GuardrailsMode::Required => anyhow::bail!(
                "guardrails file '{path}' not found; set SYNAPSE_GUARDRAILS=optional to run without guardrails"
            ),
            GuardrailsMode::Optional => {
                tracing::warn!(path, "guardrails file absent; guardrails are off");
                Ok(())
            }
        };
    };
    validate_policy_keys(cfg, routes)?;
    let undefined: Vec<String> = routes
        .route_policies()
        .into_iter()
        .map(|(a, p)| ("route", a, p))
        .chain(
            embed_routes
                .route_policies()
                .into_iter()
                .map(|(a, p)| ("embedding", a, p)),
        )
        .filter(|(_, _, p)| !cfg.guardrails.contains_key(*p))
        .map(|(kind, alias, policy)| format!("{kind} '{alias}' names undefined policy '{policy}'"))
        .collect();
    match (undefined.is_empty(), mode) {
        (true, _) => Ok(()),
        (false, GuardrailsMode::Required) => anyhow::bail!(
            "{} in '{path}'; define them or set SYNAPSE_GUARDRAILS=optional",
            undefined.join("; ")
        ),
        (false, GuardrailsMode::Optional) => {
            undefined
                .iter()
                .for_each(|u| tracing::warn!("{u}; it is not scanned"));
            Ok(())
        }
    }
}

const MASKABLE: [&str; 3] = ["secrets", "pii", "ban_substrings"];
/// Scanners `output_scanners` rejects, with the reason shown in the error.
const INPUT_ONLY: [(&str, &str); 3] = [
    ("token_limit", ""),
    ("role_override", ""),
    ("prompt_injection", " (it includes role_override)"),
];

/// Fails on a policy key the engine cannot honour, naming the policy and key.
pub fn validate_policy_keys(cfg: &GuardrailsConfig, routes: &RouteTable) -> anyhow::Result<()> {
    cfg.guardrails.iter().try_for_each(|(name, p)| {
        check_policy(name, p)?;
        if let Some(c) = &p.classifier {
            check_classifier(name, c, routes)?;
        }
        Ok(())
    })
}

/// The checks on one policy that need no route table.
pub(crate) fn check_policy(name: &str, p: &Policy) -> anyhow::Result<()> {
    check_masks(name, "scanners", &p.scanners)?;
    check_masks(name, "output_scanners", &p.output_scanners)?;
    if let Some((kind, why)) = p
        .output_scanners
        .iter()
        .find_map(|s| INPUT_ONLY.iter().find(|(k, _)| *k == s.kind()))
    {
        anyhow::bail!("policy '{name}': output_scanners cannot list '{kind}'{why}");
    }
    if p.output_holdback_chars == 0 {
        anyhow::bail!("policy '{name}': output_holdback_chars must be at least 1");
    }
    Ok(())
}

fn check_masks(policy: &str, key: &str, specs: &[ScannerSpec]) -> anyhow::Result<()> {
    match specs
        .iter()
        .find(|s| s.action() == Some(Action::Mask) && !MASKABLE.contains(&s.kind()))
    {
        Some(s) => anyhow::bail!(
            "policy '{policy}': {key} scanner '{}' cannot use action \"mask\" (only secrets, pii, ban_substrings)",
            s.kind()
        ),
        None => Ok(()),
    }
}

fn check_classifier(policy: &str, c: &ClassifierConfig, routes: &RouteTable) -> anyhow::Result<()> {
    let legs = routes.legs(&c.route).ok_or_else(|| {
        anyhow::anyhow!(
            "policy '{policy}': classifier route '{}' is not defined",
            c.route
        )
    })?;
    if !legs.iter().any(|l| l.provider == "typesafe") {
        anyhow::bail!(
            "policy '{policy}': classifier route '{}' has no typesafe leg",
            c.route
        );
    }
    if c.questions.is_empty() {
        anyhow::bail!("policy '{policy}': classifier needs at least one entry in questions");
    }
    c.questions.iter().try_for_each(|(q, spec)| {
        if !(0.0..=1.0).contains(&spec.threshold) {
            anyhow::bail!(
                "policy '{policy}': classifier question '{q}' threshold must be within 0..1"
            );
        }
        if spec.action == Some(Action::Mask) {
            anyhow::bail!(
                "policy '{policy}': classifier question '{q}' cannot use action \"mask\""
            );
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routes() -> RouteTable {
        RouteTable::from_toml_str(
            r#"
            [routes."a"]
            policy = "strict"
            legs = [{ provider = "vertex", model = "m" }]
            [routes."b"]
            legs = [{ provider = "vertex", model = "m" }]
        "#,
        )
        .unwrap()
    }

    fn embeds() -> EmbeddingRouteTable {
        EmbeddingRouteTable::from_toml_str(
            r#"[embeddings."e"]
               dimensions = 4
               policy = "embed_only"
               legs = [{ provider = "vertex", model = "x" }]"#,
        )
        .unwrap()
    }

    fn cfg(toml: &str) -> GuardrailsConfig {
        GuardrailsConfig::from_toml_str(toml).unwrap()
    }

    const BOTH: &str = "[guardrails.strict]\nscanners = []\n[guardrails.embed_only]\nscanners = []";

    #[test]
    fn required_without_a_file_fails_and_optional_passes() {
        let err = validate_guardrails(
            GuardrailsMode::Required,
            "g.toml",
            None,
            &routes(),
            &embeds(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("g.toml") && err.contains("SYNAPSE_GUARDRAILS"),
            "{err}"
        );
        assert!(validate_guardrails(
            GuardrailsMode::Optional,
            "g.toml",
            None,
            &routes(),
            &embeds()
        )
        .is_ok());
    }

    #[test]
    fn undefined_policies_fail_under_required_and_pass_under_optional() {
        let only_strict = cfg("[guardrails.strict]\nscanners = []");
        let err = validate_guardrails(
            GuardrailsMode::Required,
            "g.toml",
            Some(&only_strict),
            &routes(),
            &embeds(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("embedding 'e'") && err.contains("embed_only"),
            "{err}"
        );
        assert!(validate_guardrails(
            GuardrailsMode::Optional,
            "g.toml",
            Some(&only_strict),
            &routes(),
            &embeds()
        )
        .is_ok());
    }

    #[test]
    fn undefined_chat_route_policy_fails_under_required() {
        let only_embed = cfg("[guardrails.embed_only]\nscanners = []");
        let err = validate_guardrails(
            GuardrailsMode::Required,
            "g.toml",
            Some(&only_embed),
            &routes(),
            &embeds(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("route 'a'") && err.contains("'strict'"),
            "{err}"
        );
    }

    #[test]
    fn every_policy_defined_passes_even_without_default() {
        assert!(validate_guardrails(
            GuardrailsMode::Required,
            "g.toml",
            Some(&cfg(BOTH)),
            &routes(),
            &embeds()
        )
        .is_ok());
    }

    fn routes_with_classifier() -> RouteTable {
        RouteTable::from_toml_str(
            r#"[routes."guard-classifier"]
               legs = [{ provider = "typesafe", model = "jev-latest" }]
               [routes."fast"]
               legs = [{ provider = "qwen", model = "qwen-max" }]"#,
        )
        .unwrap()
    }

    fn check(toml: &str) -> anyhow::Result<()> {
        validate_policy_keys(
            &GuardrailsConfig::from_toml_str(toml).unwrap(),
            &routes_with_classifier(),
        )
    }

    #[test]
    fn mask_is_accepted_on_secrets_pii_and_ban_substrings() {
        check(
            r#"[guardrails.p]
            scanners = [{ type = "secrets", action = "mask" }, { type = "pii", action = "mask" },
                        { type = "ban_substrings", substrings = ["x"], action = "mask" }]"#,
        )
        .unwrap();
    }

    #[test]
    fn mask_on_other_scanners_fails_naming_policy_and_scanner() {
        let err = check(
            r#"[guardrails.p]
            scanners = [{ type = "token_limit", max_chars = 10, action = "mask" }]"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("'p'") && err.contains("token_limit") && err.contains("mask"),
            "{err}"
        );
    }

    #[test]
    fn output_scanners_reject_token_limit_and_role_override() {
        for kind in [
            "{ type = \"token_limit\", max_chars = 10 }",
            "\"role_override\"",
        ] {
            let err = check(&format!(
                "[guardrails.p]\nscanners = []\noutput_scanners = [{kind}]"
            ))
            .unwrap_err()
            .to_string();
            assert!(err.contains("output_scanners"), "{err}");
        }
    }

    #[test]
    fn output_scanners_reject_prompt_injection_naming_role_override() {
        let err = check("[guardrails.p]\nscanners = []\noutput_scanners = [\"prompt_injection\"]")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("'p'")
                && err.contains("prompt_injection")
                && err.contains("role_override"),
            "{err}"
        );
    }

    #[test]
    fn holdback_must_be_positive() {
        let err = check("[guardrails.p]\nscanners = []\noutput_holdback_chars = 0")
            .unwrap_err()
            .to_string();
        assert!(err.contains("output_holdback_chars"), "{err}");
    }

    #[test]
    fn holdback_alone_is_accepted() {
        check("[guardrails.p]\nscanners = []\noutput_holdback_chars = 64").unwrap();
    }

    #[test]
    fn classifier_route_must_exist_and_have_a_typesafe_leg() {
        let base = |route: &str| {
            format!(
                "[guardrails.p]\nscanners = []\n[guardrails.p.classifier]\nroute = \"{route}\"\n\
                 [guardrails.p.classifier.questions.q]\ninstructions = \"x\"\nthreshold = 0.5"
            )
        };
        check(&base("guard-classifier")).unwrap();
        assert!(check(&base("missing"))
            .unwrap_err()
            .to_string()
            .contains("missing"));
        assert!(check(&base("fast"))
            .unwrap_err()
            .to_string()
            .contains("typesafe"));
    }

    #[test]
    fn classifier_threshold_range_action_and_questions_are_checked() {
        let q = |body: &str| {
            format!(
                "[guardrails.p]\nscanners = []\n[guardrails.p.classifier]\nroute = \"guard-classifier\"\n{body}"
            )
        };
        let bad_threshold =
            q("[guardrails.p.classifier.questions.q]\ninstructions = \"x\"\nthreshold = 1.5");
        assert!(check(&bad_threshold)
            .unwrap_err()
            .to_string()
            .contains("threshold"));
        let mask = q("[guardrails.p.classifier.questions.q]\ninstructions = \"x\"\nthreshold = 0.5\naction = \"mask\"");
        assert!(check(&mask).unwrap_err().to_string().contains("mask"));
        let none = q("questions = {}");
        assert!(check(&none).unwrap_err().to_string().contains("questions"));
    }
}
