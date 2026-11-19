// SPDX-License-Identifier: EUPL-1.2
//! The KeyValuePair arrays an endpoint carries (`receiverInfo`, `notifierInfo`,
//! a Registration's `contextSourceInfo`): the headers a pair may not name, and
//! the credential values a read masks and a write-back restores. A leaf module,
//! so Subscriptions, Registrations and snapshots share it without a cycle.

use antares_model::NgsiError;
use serde_json::Value;

/// Headers the notification client writes itself (RFC 7230 3.3, 5.4, 6.1),
/// which a `receiverInfo` pair may not name.
pub(crate) const CLIENT_OWNED_HEADERS: [&str; 10] = [
    "Host",
    "Content-Length",
    "Transfer-Encoding",
    "Connection",
    "Keep-Alive",
    "Proxy-Connection",
    "TE",
    "Trailer",
    "Upgrade",
    "Expect",
];

/// What a read answers in place of each `endpoint.receiverInfo` value.
pub(crate) const MASKED: &str = "***";

/// Each `value` of the KeyValuePair array `pairs` replaced by [`MASKED`],
/// except a pair `keep(key, value)` admits: a read answers that a credential
/// is set, never the credential.
pub(crate) fn mask_pairs(pairs: &mut Value, keep: impl Fn(&str, &str) -> bool) {
    for pair in pairs.as_array_mut().into_iter().flatten() {
        let kept = pair
            .get("key")
            .and_then(Value::as_str)
            .zip(pair.get("value").and_then(Value::as_str))
            .is_some_and(|(k, v)| keep(k, v));
        if let (false, Some(v)) = (kept, pair.get_mut("value")) {
            *v = Value::String(MASKED.into());
        }
    }
}

/// The stored value back in place of each [`MASKED`] value of `pairs` whose
/// key the stored array `held` carries, so a client that writes back what a
/// read answered keeps the credential instead of storing the mask. A
/// credential stays with the endpoint it was given for: with `moved` (the
/// write also changes the endpoint) a masked value is refused.
pub(crate) fn unmask_pairs(
    member: &str,
    pairs: &mut Value,
    held: Option<&Value>,
    moved: bool,
) -> Result<(), NgsiError> {
    let held = held
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for pair in pairs.as_array_mut().into_iter().flatten() {
        if pair.get("value").and_then(Value::as_str) != Some(MASKED) {
            continue;
        }
        let key = pair.get("key").cloned();
        let previous = held
            .iter()
            .find(|h| h.get("key") == key.as_ref())
            .and_then(|h| h.get("value"))
            .cloned();
        if let (Some(previous), Some(v)) = (previous, pair.get_mut("value")) {
            if moved {
                return Err(NgsiError::BadRequestData(format!(
                    "{member}: a masked value cannot move to another endpoint: send the \
                     credential again"
                )));
            }
            *v = previous;
        }
    }
    Ok(())
}
