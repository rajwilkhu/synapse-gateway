//! The text leaves a guard scans on each lane, as mutable references so masks
//! write back in place. Each segment is scanned as its leaves joined with
//! `SEPARATOR`.

use serde_json::{Map, Value};

use crate::routing::request::ChatRequest;

pub type Segment<'a> = Vec<&'a mut String>;

pub const SEPARATOR: &str = "\n";

/// A segment's scan text.
pub fn joined(seg: &[&mut String]) -> String {
    seg.iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

/// One segment: the text of `system`, `user` and `tool` messages, string
/// content or the `text` of content parts. A message whose text is empty
/// contributes nothing.
pub fn chat_leaves(req: &mut ChatRequest) -> Vec<Segment<'_>> {
    vec![req
        .messages
        .iter_mut()
        .filter(|m| matches!(m.role.as_str(), "system" | "user" | "tool"))
        .flat_map(|m| content_leaves(&mut m.content))
        .collect()]
}

fn content_leaves(content: &mut Value) -> Vec<&mut String> {
    match content {
        Value::String(s) if !s.is_empty() => vec![s],
        Value::Array(parts) => {
            let leaves: Vec<&mut String> = parts
                .iter_mut()
                .filter_map(|p| match p.get_mut("text") {
                    Some(Value::String(s)) => Some(s),
                    _ => None,
                })
                .collect();
            let blank = leaves.iter().all(|s| s.is_empty()) && leaves.len() <= 1;
            if blank {
                Vec::new()
            } else {
                leaves
            }
        }
        _ => Vec::new(),
    }
}

/// One segment for a Gemini request body: the system instruction's text, plus
/// every non-model turn's text parts and the string leaves of its function
/// responses. Turns come from `contents` (generation) and `content`
/// (embedding); a batch body's `requests` are read the same way. Model turns
/// in `contents` (any case), function calls and inline or file data are
/// skipped; embedding `content` is read whatever its role. camelCase
/// and snake_case keys are both read, and both when both are present. A
/// repeated field may be an array or a single object.
pub fn gemini_leaves(body: &mut Value) -> Vec<Segment<'_>> {
    vec![gemini_text(body)]
}

fn gemini_text(body: &mut Value) -> Vec<&mut String> {
    let Some(obj) = body.as_object_mut() else {
        return Vec::new();
    };
    let [camel, snake, contents, content, requests] = pick(
        obj,
        [
            "systemInstruction",
            "system_instruction",
            "contents",
            "content",
            "requests",
        ],
    );
    let system = [camel, snake]
        .into_iter()
        .flatten()
        .filter_map(|s| s.get_mut("parts"))
        .flat_map(parts_leaves);
    let turns = items_mut(contents)
        .filter(|c| !is_model_turn(c))
        .chain(items_mut(content))
        .filter_map(|c| c.get_mut("parts"))
        .flat_map(parts_leaves);
    let batch = items_mut(requests).flat_map(gemini_text);
    system.chain(turns).chain(batch).collect()
}

/// Keys whose values are encoded media or media references, not text.
const MEDIA_KEYS: &[&str] = &[
    "bytesBase64Encoded",
    "gcsUri",
    "inlineData",
    "inline_data",
    "fileData",
    "file_data",
];

/// One segment of every string leaf in a body whose shape is not known, such
/// as a Vertex `predict` body. Numbers, booleans, object keys and values under
/// encoded-media keys (`bytesBase64Encoded`, `gcsUri`, `inlineData`,
/// `fileData`) are skipped.
pub fn body_leaves(body: &mut Value) -> Vec<Segment<'_>> {
    vec![text_leaves(body)]
}

fn text_leaves(v: &mut Value) -> Vec<&mut String> {
    match v {
        Value::String(s) => vec![s],
        Value::Array(items) => items.iter_mut().flat_map(text_leaves).collect(),
        Value::Object(map) => map
            .iter_mut()
            .filter(|(k, _)| !MEDIA_KEYS.contains(&k.as_str()))
            .flat_map(|(_, v)| text_leaves(v))
            .collect(),
        _ => Vec::new(),
    }
}

/// One segment for a Jev System One body: every string leaf of `state` and
/// `questions`. Numbers, booleans and object keys are skipped.
pub fn jev_leaves(body: &mut Value) -> Vec<Segment<'_>> {
    let Some(obj) = body.as_object_mut() else {
        return vec![Vec::new()];
    };
    let [state, questions] = pick(obj, ["state", "questions"]);
    vec![[state, questions]
        .into_iter()
        .flatten()
        .flat_map(string_leaves)
        .collect()]
}

/// One segment per embedding input.
pub fn embedding_leaves(inputs: &mut [String]) -> Vec<Segment<'_>> {
    inputs.iter_mut().map(|s| vec![s]).collect()
}

/// The values under `keys`, in `keys` order.
fn pick<'a, const N: usize>(
    obj: &'a mut Map<String, Value>,
    keys: [&str; N],
) -> [Option<&'a mut Value>; N] {
    let mut slots: [Option<&'a mut Value>; N] = std::array::from_fn(|_| None);
    for (k, v) in obj.iter_mut() {
        if let Some(i) = keys.iter().position(|key| key == k) {
            slots[i] = Some(v);
        }
    }
    slots
}

fn is_model_turn(c: &Value) -> bool {
    c.get("role")
        .and_then(Value::as_str)
        .is_some_and(|r| r.eq_ignore_ascii_case("model"))
}

/// The elements of a repeated field sent as an array, or the field itself when
/// sent as a single object.
fn items_mut(v: Option<&mut Value>) -> Box<dyn Iterator<Item = &mut Value> + '_> {
    match v {
        Some(Value::Array(a)) => Box::new(a.iter_mut()),
        Some(o @ Value::Object(_)) => Box::new(std::iter::once(o)),
        _ => Box::new(std::iter::empty()),
    }
}

fn parts_leaves(parts: &mut Value) -> Vec<&mut String> {
    items_mut(Some(parts))
        .flat_map(|p| {
            let Some(obj) = p.as_object_mut() else {
                return Vec::new();
            };
            let [text, camel, snake] = pick(obj, ["text", "functionResponse", "function_response"]);
            let text = text.and_then(|t| match t {
                Value::String(s) => Some(s),
                _ => None,
            });
            let tool = [camel, snake]
                .into_iter()
                .flatten()
                .filter_map(|f| f.get_mut("response"))
                .flat_map(string_leaves);
            text.into_iter().chain(tool).collect()
        })
        .collect()
}

fn string_leaves(v: &mut Value) -> Vec<&mut String> {
    match v {
        Value::String(s) => vec![s],
        Value::Array(items) => items.iter_mut().flat_map(string_leaves).collect(),
        Value::Object(map) => map.values_mut().flat_map(string_leaves).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::routing::request::ChatRequest;

    fn one(v: Vec<Segment<'_>>) -> String {
        assert_eq!(v.len(), 1);
        joined(&v[0])
    }

    #[test]
    fn gemini_scans_system_user_text_and_tool_results() {
        let mut body = json!({
            "systemInstruction": {"parts": [{"text": "SYS"}]},
            "contents": [
                {"role": "user", "parts": [{"text": "USER"}, {"inlineData": {"mimeType": "image/png", "data": "QUJD"}}]},
                {"role": "model", "parts": [{"text": "MODEL"}, {"functionCall": {"name": "f", "args": {"q": "CALLARG"}}}]},
                {"parts": [{"text": "NOROLE"}]},
                {"role": "user", "parts": [{"functionResponse": {"name": "f", "response": {"out": ["TOOL1", {"deep": "TOOL2"}], "n": 3}}}]},
                {"role": "function", "parts": [{"functionResponse": {"name": "f", "response": {"out": "LEGACY"}}}]}
            ]
        });
        let s = one(gemini_leaves(&mut body));
        for want in ["SYS", "USER", "NOROLE", "TOOL1", "TOOL2", "LEGACY"] {
            assert!(s.contains(want), "missing {want} in {s}");
        }
        for skip in ["MODEL", "CALLARG", "QUJD"] {
            assert!(!s.contains(skip), "should not scan {skip}: {s}");
        }
    }

    #[test]
    fn gemini_accepts_snake_case_keys() {
        let mut body = json!({
            "system_instruction": {"parts": [{"text": "SYS"}]},
            "contents": [{"role": "user", "parts": [{"function_response": {"name": "f", "response": {"out": "TOOL"}}}]}]
        });
        let s = one(gemini_leaves(&mut body));
        assert!(s.contains("SYS") && s.contains("TOOL"), "{s}");
    }

    #[test]
    fn gemini_scans_both_spellings_when_both_are_present() {
        let mut body = json!({
            "systemInstruction": {"parts": [{"text": "CLEANSYS"}]},
            "system_instruction": {"parts": [{"text": "MARKSYS"}]},
            "contents": [{"role": "user", "parts": [{
                "functionResponse": {"name": "f", "response": {"out": "CLEANTOOL"}},
                "function_response": {"name": "f", "response": {"out": "MARKTOOL"}}
            }]}]
        });
        let s = one(gemini_leaves(&mut body));
        for want in ["CLEANSYS", "MARKSYS", "CLEANTOOL", "MARKTOOL"] {
            assert!(s.contains(want), "missing {want} in {s}");
        }
    }

    #[test]
    fn gemini_scans_single_object_contents_and_parts() {
        let mut body = json!({
            "systemInstruction": {"parts": {"text": "SYS"}},
            "contents": {"role": "user", "parts": {"text": "USER"}}
        });
        let s = one(gemini_leaves(&mut body));
        assert!(s.contains("SYS") && s.contains("USER"), "{s}");
    }

    #[test]
    fn gemini_roles_match_case_insensitively() {
        let mut body = json!({"contents": [
            {"role": "USER", "parts": [{"text": "UPPERUSER"}]},
            {"role": "MODEL", "parts": [{"text": "UPPERMODEL"}]},
            {"role": "Model", "parts": [{"text": "MIXEDMODEL"}]}
        ]});
        let s = one(gemini_leaves(&mut body));
        assert!(s.contains("UPPERUSER"), "{s}");
        assert!(
            !s.contains("UPPERMODEL") && !s.contains("MIXEDMODEL"),
            "{s}"
        );
    }

    #[test]
    fn gemini_scans_embed_and_batch_request_bodies() {
        let mut embed = json!({"content": {"parts": [{"text": "EMBED"}]}});
        assert!(one(gemini_leaves(&mut embed)).contains("EMBED"));
        let mut batch = json!({"requests": [
            {"content": {"parts": [{"text": "BATCHEMBED"}]}},
            {"contents": [{"role": "user", "parts": [{"text": "BATCHGEN"}]}]}
        ]});
        let s = one(gemini_leaves(&mut batch));
        assert!(s.contains("BATCHEMBED") && s.contains("BATCHGEN"), "{s}");
    }

    #[test]
    fn gemini_scans_embed_content_whatever_its_role() {
        let mut embed = json!({"content": {"role": "model", "parts": [{"text": "EMBED"}]}});
        assert!(one(gemini_leaves(&mut embed)).contains("EMBED"));
        let mut batch =
            json!({"requests": [{"content": {"role": "model", "parts": [{"text": "BATCH"}]}}]});
        assert!(one(gemini_leaves(&mut batch)).contains("BATCH"));
    }

    #[test]
    fn body_leaves_scan_every_string_leaf() {
        let mut body = json!({"instances": [{"content": "A", "n": 1}], "parameters": {"k": "B"}});
        let s = one(body_leaves(&mut body));
        assert!(s.contains('A') && s.contains('B'), "{s}");
    }

    #[test]
    fn body_leaves_skip_encoded_media() {
        let mut body = json!({"instances": [{
            "text": "CAPTION",
            "image": {"bytesBase64Encoded": "QUJDIMAGE"},
            "video": {"gcsUri": "gs://b/VIDEO.mp4"},
            "part": {"inlineData": {"mimeType": "image/png", "data": "QUJDINLINE"}},
            "file": {"fileData": {"fileUri": "gs://b/FILE.pdf"}}
        }]});
        let s = one(body_leaves(&mut body));
        assert!(s.contains("CAPTION"), "{s}");
        for skip in ["QUJDIMAGE", "VIDEO", "QUJDINLINE", "FILE"] {
            assert!(!s.contains(skip), "should not scan {skip}: {s}");
        }
    }

    #[test]
    fn gemini_body_without_contents_is_one_empty_segment() {
        assert_eq!(one(gemini_leaves(&mut json!({"cachedContent": "c/1"}))), "");
        assert_eq!(
            one(gemini_leaves(
                &mut json!({"contents": [{"role": "user", "parts": []}]})
            )),
            ""
        );
    }

    #[test]
    fn writing_a_leaf_changes_the_body() {
        let mut body = json!({"contents": [{"role": "user", "parts": [{"text": "a"},
            {"functionResponse": {"name": "f", "response": {"out": "b"}}}]}]});
        for leaf in gemini_leaves(&mut body).into_iter().flatten() {
            leaf.push('!');
        }
        assert_eq!(body["contents"][0]["parts"][0]["text"], "a!");
        assert_eq!(
            body["contents"][0]["parts"][1]["functionResponse"]["response"]["out"],
            "b!"
        );
    }

    #[test]
    fn chat_leaves_cover_system_user_tool_text_only() {
        let mut req: ChatRequest = serde_json::from_value(json!({"model": "m", "messages": [
            {"role": "system", "content": "SYS"},
            {"role": "assistant", "content": "ASSIST"},
            {"role": "user", "content": [{"type": "text", "text": "USER"},
                                         {"type": "image_url", "image_url": {"url": "IMG"}}]},
            {"role": "tool", "tool_call_id": "t", "content": "TOOL"}
        ]}))
        .unwrap();
        assert_eq!(one(chat_leaves(&mut req)), "SYS\nUSER\nTOOL");
    }

    #[test]
    fn chat_leaves_join_like_the_scanned_text() {
        let mut req: ChatRequest = serde_json::from_value(json!({"model": "m", "messages": [
            {"role": "user", "content": ""},
            {"role": "user", "content": [{"text": ""}]},
            {"role": "user", "content": [{"text": ""}, {"text": ""}]},
            {"role": "user", "content": [{"text": "a"}, {"text": ""}, {"text": "b"}]}
        ]}))
        .unwrap();
        assert_eq!(one(chat_leaves(&mut req)), "\n\na\n\nb");
    }

    #[test]
    fn embedding_inputs_are_one_segment_each() {
        let mut inputs = vec!["a".to_string(), "b".to_string()];
        let segs = embedding_leaves(&mut inputs);
        assert_eq!(segs.len(), 2);
        assert_eq!(joined(&segs[1]), "b");
    }

    #[test]
    fn jev_scans_string_leaves_of_state_and_questions_only() {
        let mut body = json!({
            "model": "MODELNAME",
            "state": {"ticket": "STATE", "n": 4, "ok": true},
            "questions": {"KEYNAME": {"type": "noul", "instructions": "QUESTION"}}
        });
        let s = one(jev_leaves(&mut body));
        for want in ["STATE", "QUESTION", "noul"] {
            assert!(s.contains(want), "missing {want} in {s}");
        }
        for skip in ["MODELNAME", "KEYNAME", "4", "true"] {
            assert!(!s.contains(skip), "should not scan {skip}: {s}");
        }
        assert!(one(jev_leaves(&mut json!({"state": "PLAIN"}))).contains("PLAIN"));
    }
}
