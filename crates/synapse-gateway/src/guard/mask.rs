//! What a match does under a policy, and rewriting mask spans in the leaves
//! of a segment.

use std::ops::Range;

use llm_guard::Severity;

use super::leaves::SEPARATOR;
use super::policy::{Action, PolicyMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Block,
    Mask,
    Record,
}

/// The wanted action (explicit, else from severity) and what it does under `mode`.
pub fn effect(action: Option<Action>, severity: Severity, mode: PolicyMode) -> (Action, Effect) {
    let wanted = action.unwrap_or(match severity {
        Severity::Block => Action::Block,
        _ => Action::Log,
    });
    let effect = match (wanted, mode) {
        (Action::Log, _) | (_, PolicyMode::Observe) => Effect::Record,
        (Action::Block, PolicyMode::Block) => Effect::Block,
        (Action::Mask, PolicyMode::Block) => Effect::Mask,
    };
    (wanted, effect)
}

/// The `outcome` metric label for one scan.
pub fn outcome_label(wanted: &[Action], effects: &[Effect]) -> &'static str {
    if effects.contains(&Effect::Block) {
        "block"
    } else if effects.contains(&Effect::Mask) {
        "masked"
    } else if wanted.iter().any(|a| *a != Action::Log) {
        "observe"
    } else if !wanted.is_empty() {
        "flag"
    } else {
        "pass"
    }
}

/// A byte range of a segment's joined text to redact. `order` is the
/// scanner's position in the policy; the lowest names a merged span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskSpan {
    pub range: Range<usize>,
    pub label: &'static str,
    pub order: usize,
}

/// Sorted, non-overlapping spans.
pub fn merge(mut spans: Vec<MaskSpan>) -> Vec<MaskSpan> {
    spans.sort_by_key(|s| s.range.start);
    spans
        .into_iter()
        .fold(Vec::new(), |mut out: Vec<MaskSpan>, s| {
            match out.last_mut() {
                Some(last) if s.range.start < last.range.end => {
                    last.range.end = last.range.end.max(s.range.end);
                    if s.order < last.order {
                        last.order = s.order;
                        last.label = s.label;
                    }
                }
                _ => out.push(s),
            }
            out
        })
}

/// Where offset `p` (into the unmasked text) lands once `spans` are applied;
/// an offset inside a span lands at the span's start.
pub fn shifted(spans: &[MaskSpan], p: usize) -> usize {
    let mut delta = 0isize;
    for s in spans {
        if s.range.start >= p {
            break;
        }
        let repl = "[REDACTED:]".len() + s.label.len();
        if s.range.end > p {
            return (s.range.start as isize + delta) as usize;
        }
        delta += repl as isize - s.range.len() as isize;
    }
    (p as isize + delta) as usize
}

/// Replace each leaf's share of every merged span with `[REDACTED:<label>]`.
/// Span offsets are into the leaves joined with `SEPARATOR`.
pub fn apply(leaves: &mut [&mut String], spans: &[MaskSpan]) {
    let mut start = 0;
    for leaf in leaves.iter_mut() {
        let own = start..start + leaf.len();
        start = own.end + SEPARATOR.len();
        for s in spans.iter().rev() {
            let (from, to) = (s.range.start.max(own.start), s.range.end.min(own.end));
            if from < to {
                leaf.replace_range(
                    from - own.start..to - own.start,
                    &format!("[REDACTED:{}]", s.label),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_guard::Severity;

    fn span(r: Range<usize>, label: &'static str, order: usize) -> MaskSpan {
        MaskSpan {
            range: r,
            label,
            order,
        }
    }

    #[test]
    fn unset_action_follows_severity_and_mode() {
        assert_eq!(
            effect(None, Severity::Block, PolicyMode::Block),
            (Action::Block, Effect::Block)
        );
        assert_eq!(
            effect(None, Severity::Warn, PolicyMode::Block),
            (Action::Log, Effect::Record)
        );
        assert_eq!(
            effect(None, Severity::Block, PolicyMode::Observe),
            (Action::Block, Effect::Record)
        );
    }

    #[test]
    fn explicit_action_overrides_severity() {
        assert_eq!(
            effect(Some(Action::Block), Severity::Warn, PolicyMode::Block).1,
            Effect::Block
        );
        assert_eq!(
            effect(Some(Action::Mask), Severity::Block, PolicyMode::Block).1,
            Effect::Mask
        );
        assert_eq!(
            effect(Some(Action::Log), Severity::Block, PolicyMode::Block).1,
            Effect::Record
        );
        assert_eq!(
            effect(Some(Action::Mask), Severity::Warn, PolicyMode::Observe).1,
            Effect::Record
        );
    }

    #[test]
    fn outcome_labels() {
        assert_eq!(outcome_label(&[], &[]), "pass");
        assert_eq!(outcome_label(&[Action::Log], &[Effect::Record]), "flag");
        assert_eq!(
            outcome_label(&[Action::Block], &[Effect::Record]),
            "observe"
        );
        assert_eq!(outcome_label(&[Action::Mask], &[Effect::Mask]), "masked");
        assert_eq!(
            outcome_label(
                &[Action::Mask, Action::Block],
                &[Effect::Mask, Effect::Block]
            ),
            "block"
        );
    }

    #[test]
    fn overlapping_spans_merge_under_the_first_scanner() {
        let merged = merge(vec![
            span(5..12, "pii_patterns", 1),
            span(0..8, "secrets", 0),
            span(20..22, "pii_patterns", 1),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].range, 0..12);
        assert_eq!(merged[0].label, "secrets");
        assert_eq!(merged[1].range, 20..22);
    }

    #[test]
    fn apply_replaces_right_to_left_in_one_leaf() {
        let mut a = "key sk-1 and sk-2".to_string();
        apply(
            &mut [&mut a],
            &[span(4..8, "secrets", 0), span(13..17, "secrets", 0)],
        );
        assert_eq!(a, "key [REDACTED:secrets] and [REDACTED:secrets]");
    }

    #[test]
    fn span_across_two_leaves_masks_both() {
        let (mut a, mut b) = ("abc SEC".to_string(), "RET def".to_string());
        // joined: "abc SEC\nRET def"; the span covers "SEC\nRET" = 4..11
        apply(&mut [&mut a, &mut b], &[span(4..11, "secrets", 0)]);
        assert_eq!(a, "abc [REDACTED:secrets]");
        assert_eq!(b, "[REDACTED:secrets] def");
    }

    #[test]
    fn mask_keeps_multibyte_neighbours() {
        let mut a = "héllo sk-1 wörld".to_string();
        let start = a.find("sk-1").unwrap();
        apply(&mut [&mut a], &[span(start..start + 4, "secrets", 0)]);
        assert_eq!(a, "héllo [REDACTED:secrets] wörld");
    }

    #[test]
    fn later_leaves_use_original_offsets() {
        let (mut a, mut b) = ("sk-1".to_string(), "sk-2".to_string());
        apply(
            &mut [&mut a, &mut b],
            &[span(0..4, "secrets", 0), span(5..9, "secrets", 0)],
        );
        assert_eq!(
            (a.as_str(), b.as_str()),
            ("[REDACTED:secrets]", "[REDACTED:secrets]")
        );
    }
}
