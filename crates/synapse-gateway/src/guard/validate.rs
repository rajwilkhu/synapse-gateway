//! Startup checks that the guardrails file and every policy a route names exist.

use crate::config::GuardrailsMode;
use crate::routing::embeddings::EmbeddingRouteTable;
use crate::routing::table::RouteTable;

use super::policy::GuardrailsConfig;

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
}
