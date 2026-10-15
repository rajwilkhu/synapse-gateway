//! The text a guard scans on lanes whose body is not an OpenAI chat request.

use serde_json::Value;

/// One segment for a Gemini `generateContent` body: the system instruction's
/// text, plus every non-model turn's text parts and the string leaves of its
/// function responses. Model turns (any case), function calls and inline or
/// file data are skipped. camelCase and snake_case keys are both read, and both
/// when both are present. A repeated field may be an array or a single object.
pub fn gemini_segments(body: &Value) -> Vec<String> {
    let system = field(body, "systemInstruction", "system_instruction")
        .filter_map(|s| s.get("parts"))
        .flat_map(parts_text);
    let turns = items(body.get("contents"))
        .filter(|c| {
            !c.get("role")
                .and_then(Value::as_str)
                .is_some_and(|r| r.eq_ignore_ascii_case("model"))
        })
        .filter_map(|c| c.get("parts"))
        .flat_map(parts_text);
    vec![system
        .into_iter()
        .chain(turns)
        .collect::<Vec<_>>()
        .join("\n")]
}

/// One segment for a Jev System One body: every string leaf of `state` and
/// `questions`. Numbers, booleans and object keys are skipped.
pub fn jev_segments(body: &Value) -> Vec<String> {
    let leaves: Vec<String> = ["state", "questions"]
        .iter()
        .filter_map(|k| body.get(*k))
        .flat_map(string_leaves)
        .collect();
    vec![leaves.join("\n")]
}

fn field<'a>(v: &'a Value, camel: &str, snake: &str) -> impl Iterator<Item = &'a Value> {
    [v.get(camel), v.get(snake)].into_iter().flatten()
}

/// The elements of a repeated field sent as an array, or the field itself when
/// sent as a single object.
fn items(v: Option<&Value>) -> impl Iterator<Item = &Value> {
    let (list, single) = match v {
        Some(Value::Array(a)) => (a.as_slice(), None),
        Some(o @ Value::Object(_)) => (&[][..], Some(o)),
        _ => (&[][..], None),
    };
    list.iter().chain(single)
}

fn parts_text(parts: &Value) -> Vec<String> {
    items(Some(parts))
        .flat_map(|p| {
            let text = p.get("text").and_then(Value::as_str).map(str::to_string);
            let tool = field(p, "functionResponse", "function_response")
                .filter_map(|f| f.get("response"))
                .flat_map(string_leaves);
            text.into_iter().chain(tool)
        })
        .collect()
}

fn string_leaves(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items.iter().flat_map(string_leaves).collect(),
        Value::Object(map) => map.values().flat_map(string_leaves).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(v: Vec<String>) -> String {
        assert_eq!(v.len(), 1);
        v.into_iter().next().unwrap()
    }

    #[test]
    fn gemini_scans_system_user_text_and_tool_results() {
        let body = json!({
            "systemInstruction": {"parts": [{"text": "SYS"}]},
            "contents": [
                {"role": "user", "parts": [{"text": "USER"}, {"inlineData": {"mimeType": "image/png", "data": "QUJD"}}]},
                {"role": "model", "parts": [{"text": "MODEL"}, {"functionCall": {"name": "f", "args": {"q": "CALLARG"}}}]},
                {"parts": [{"text": "NOROLE"}]},
                {"role": "user", "parts": [{"functionResponse": {"name": "f", "response": {"out": ["TOOL1", {"deep": "TOOL2"}], "n": 3}}}]},
                {"role": "function", "parts": [{"functionResponse": {"name": "f", "response": {"out": "LEGACY"}}}]}
            ]
        });
        let s = one(gemini_segments(&body));
        for want in ["SYS", "USER", "NOROLE", "TOOL1", "TOOL2", "LEGACY"] {
            assert!(s.contains(want), "missing {want} in {s}");
        }
        for skip in ["MODEL", "CALLARG", "QUJD"] {
            assert!(!s.contains(skip), "should not scan {skip}: {s}");
        }
    }

    #[test]
    fn gemini_accepts_snake_case_keys() {
        let body = json!({
            "system_instruction": {"parts": [{"text": "SYS"}]},
            "contents": [{"role": "user", "parts": [{"function_response": {"name": "f", "response": {"out": "TOOL"}}}]}]
        });
        let s = one(gemini_segments(&body));
        assert!(s.contains("SYS") && s.contains("TOOL"), "{s}");
    }

    #[test]
    fn gemini_scans_both_spellings_when_both_are_present() {
        let body = json!({
            "systemInstruction": {"parts": [{"text": "CLEANSYS"}]},
            "system_instruction": {"parts": [{"text": "MARKSYS"}]},
            "contents": [{"role": "user", "parts": [{
                "functionResponse": {"name": "f", "response": {"out": "CLEANTOOL"}},
                "function_response": {"name": "f", "response": {"out": "MARKTOOL"}}
            }]}]
        });
        let s = one(gemini_segments(&body));
        for want in ["CLEANSYS", "MARKSYS", "CLEANTOOL", "MARKTOOL"] {
            assert!(s.contains(want), "missing {want} in {s}");
        }
    }

    #[test]
    fn gemini_scans_single_object_contents_and_parts() {
        let body = json!({
            "systemInstruction": {"parts": {"text": "SYS"}},
            "contents": {"role": "user", "parts": {"text": "USER"}}
        });
        let s = one(gemini_segments(&body));
        assert!(s.contains("SYS") && s.contains("USER"), "{s}");
    }

    #[test]
    fn gemini_roles_match_case_insensitively() {
        let body = json!({"contents": [
            {"role": "USER", "parts": [{"text": "UPPERUSER"}]},
            {"role": "MODEL", "parts": [{"text": "UPPERMODEL"}]},
            {"role": "Model", "parts": [{"text": "MIXEDMODEL"}]}
        ]});
        let s = one(gemini_segments(&body));
        assert!(s.contains("UPPERUSER"), "{s}");
        assert!(
            !s.contains("UPPERMODEL") && !s.contains("MIXEDMODEL"),
            "{s}"
        );
    }

    #[test]
    fn gemini_body_without_contents_is_one_empty_segment() {
        assert_eq!(
            gemini_segments(&json!({"cachedContent": "c/1"})),
            vec![String::new()]
        );
        assert_eq!(
            gemini_segments(&json!({"contents": [{"role": "user", "parts": []}]})),
            vec![String::new()]
        );
    }

    #[test]
    fn jev_scans_string_leaves_of_state_and_questions_only() {
        let body = json!({
            "model": "MODELNAME",
            "state": {"ticket": "STATE", "n": 4, "ok": true},
            "questions": {"KEYNAME": {"type": "noul", "instructions": "QUESTION"}}
        });
        let s = one(jev_segments(&body));
        for want in ["STATE", "QUESTION", "noul"] {
            assert!(s.contains(want), "missing {want} in {s}");
        }
        for skip in ["MODELNAME", "KEYNAME", "4", "true"] {
            assert!(!s.contains(skip), "should not scan {skip}: {s}");
        }
        assert!(one(jev_segments(&json!({"state": "PLAIN"}))).contains("PLAIN"));
    }
}
