//! 4.9 q= grammar: must never panic, whatever the bytes, and a parsed query
//! renders to text that parses back to the same query (T-3463), save the one
//! case render.rs documents: a pattern whose own text carries a `"`, and
//! text that escaping grows past the 4096-byte input cap (a NUL becomes
//! `\u0000`), which the parser refuses by size and nothing else.
#![no_main]
use antares_model::NgsiError;
use antares_ql::{CmpOp, QNode, QValue};
use libfuzzer_sys::fuzz_target;

fn pattern_with_quote(node: &QNode) -> bool {
    match node {
        QNode::And(items) | QNode::Or(items) => items.iter().any(pattern_with_quote),
        QNode::Cmp {
            op: CmpOp::Pattern | CmpOp::NotPattern,
            value: QValue::Str(s),
            ..
        } => s.contains('"'),
        _ => false,
    }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(node) = antares_ql::parse_q(s) {
            if pattern_with_quote(&node) {
                return;
            }
            let rendered = node.to_string();
            let again = match antares_ql::parse_q(&rendered) {
                Ok(again) => again,
                Err(NgsiError::TooComplexQuery(_)) if rendered.len() > 4096 => return,
                Err(e) => panic!("{s:?} rendered as {rendered:?}, which does not parse: {e:?}"),
            };
            assert_eq!(again, node, "{s:?} rendered as {rendered:?}");
        }
    }
});
