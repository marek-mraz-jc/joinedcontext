//! Test callers with subjects of any shape, given to the router as a mounting host gives a
//! verified one. A token of none of them is left to the router, which refuses it.

use dataroom_api::DataroomState;
use dataroom_core::Subject;

/// One test caller and the credential it presents.
#[derive(Clone)]
pub struct Caller {
    pub token: String,
    pub subject: Subject,
}

/// `state` mounted by a host that knows `callers`.
pub fn with_callers(state: DataroomState, callers: Vec<Caller>) -> DataroomState {
    state.with_subject_resolver(move |headers| {
        let presented = headers
            .get("authorization")
            .or_else(|| headers.get("x-ckan-api-key"))?
            .to_str()
            .ok()?
            .trim();
        let token = match presented.split_once(' ') {
            Some((scheme, token)) if scheme.eq_ignore_ascii_case("bearer") => token.trim(),
            _ => presented,
        };
        callers
            .iter()
            .find(|c| c.token == token)
            .map(|c| c.subject.clone())
    })
}
