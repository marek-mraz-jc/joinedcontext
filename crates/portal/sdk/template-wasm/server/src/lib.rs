//! The server half of this App: `/api/items` on the App's own host, kept in the App's own
//! schema (`migrations/`). A component holds no socket, environment, file or credential
//! (AP-147); the host answers `sql` as this App and no other, and asks the platform's `email` and
//! `ai` in the caller's name when the App's services hold them (API/06 §5).
//!
//! The builder keeps each `jc:service <name>…` block only while the App's services hold one of
//! the names, and takes the marker lines out either way (T-3709).

// jc:service ai
use jc_app_sdk::ai::{self, Completion, Message, Role};
// jc:end
// jc:service email
use jc_app_sdk::email::{self, Email, Recipients};
use jc_app_sdk::identity;
// jc:end
use jc_app_sdk::http::{Params, Request, Response, Router};
use jc_app_sdk::sql;
use serde::Deserialize;

/// The longest text an item keeps, as the table's check says.
pub const MAX_TEXT: usize = 500;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewItem {
    pub text: String,
}

/// The text to keep, trimmed, or why it is refused.
pub fn checked(item: &NewItem) -> Result<String, String> {
    let text = item.text.trim();
    if text.is_empty() {
        return Err("text is empty".into());
    }
    if text.chars().count() > MAX_TEXT {
        return Err(format!("text is longer than {MAX_TEXT} characters"));
    }
    Ok(text.to_owned())
}

// jc:service email ai
/// The newest items' texts, at most `limit`.
fn latest(limit: i64) -> Result<Vec<String>, Response> {
    let rows = sql::query(
        "select text from items order by id desc limit $1",
        &[limit.into()],
    )
    .map_err(Response::from_sql)?;
    Ok(sql::objects(&rows)
        .into_iter()
        .filter_map(|row| row.get("text")?.as_str().map(str::to_owned))
        .collect())
}
// jc:end

// jc:service email
/// The digest mail of these texts, to the person who asked for it.
pub fn digest(texts: &[String]) -> Email {
    let text = if texts.is_empty() {
        "No items yet.".to_owned()
    } else {
        texts.iter().map(|t| format!("- {t}\n")).collect()
    };
    Email {
        to: Recipients::Me,
        subject: format!("{} newest items", texts.len()),
        text,
        html: None,
    }
}
// jc:end

// jc:service ai
/// The completion that sums these texts up in one sentence.
pub fn summary_request(texts: &[String]) -> Completion {
    Completion {
        messages: vec![
            Message {
                role: Role::System,
                content: "Sum up the user's items in one short sentence.".into(),
            },
            Message {
                role: Role::User,
                content: texts.join("\n"),
            },
        ],
        max_tokens: Some(100),
        schema: None,
    }
}
// jc:end

fn list(_: &Request, _: &Params) -> Response {
    match sql::query(
        "select id, text, created_at::text as created_at from items order by id desc limit 100",
        &[],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

// jc:service email
/// Mails the caller the ten newest items; a visitor who is not signed in has no address.
fn mail_digest(_: &Request, _: &Params) -> Response {
    if identity::caller().is_none() {
        return Response::problem(401, "Unauthorized", "sign in to get the digest");
    }
    let texts = match latest(10) {
        Ok(texts) => texts,
        Err(refused) => return refused,
    };
    match email::send(&digest(&texts)) {
        Ok(id) => Response::json(202, &serde_json::json!({ "id": id })),
        Err(err) => Response::from_service(err),
    }
}
// jc:end

// jc:service ai
/// One sentence over the ten newest items from the organization's model.
fn summary(_: &Request, _: &Params) -> Response {
    let texts = match latest(10) {
        Ok(texts) => texts,
        Err(refused) => return refused,
    };
    if texts.is_empty() {
        return Response::json(200, &serde_json::json!({ "summary": "" }));
    }
    match ai::complete(&summary_request(&texts)) {
        Ok(answer) => Response::json(200, &serde_json::json!({ "summary": answer.text })),
        Err(err) => Response::from_service(err),
    }
}
// jc:end

fn create(request: &Request, _: &Params) -> Response {
    let item: NewItem = match request.json() {
        Ok(item) => item,
        Err(refused) => return refused,
    };
    let text = match checked(&item) {
        Ok(text) => text,
        Err(why) => return Response::problem(422, "Unprocessable Entity", &why),
    };
    match sql::query(
        "insert into items (text) values ($1) returning id, text, created_at::text as created_at",
        &[text.into()],
    ) {
        Ok(rows) => match sql::objects(&rows).into_iter().next() {
            Some(row) => Response::json(201, &row),
            None => Response::problem(500, "Internal Server Error", "the insert returned no row"),
        },
        Err(err) => Response::from_sql(err),
    }
}

pub fn handle(request: Request) -> Response {
    let router = Router::new()
        .get("/api/items", list)
        .post("/api/items", create);
    // jc:service email
    let router = router.post("/api/items/digest", mail_digest);
    // jc:end
    // jc:service ai
    let router = router.get("/api/items/summary", summary);
    // jc:end
    router.handle(&request)
}

jc_app_sdk::app!(handle);

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str) -> NewItem {
        NewItem { text: text.into() }
    }

    #[test]
    fn a_text_is_trimmed_and_kept() {
        assert_eq!(checked(&item("  bike lane  ")), Ok("bike lane".into()));
    }

    #[test]
    fn an_empty_or_too_long_text_is_refused() {
        assert!(checked(&item("   ")).is_err());
        assert!(checked(&item(&"é".repeat(MAX_TEXT))).is_ok());
        assert!(checked(&item(&"é".repeat(MAX_TEXT + 1))).is_err());
    }

    // jc:service email
    #[test]
    fn the_digest_goes_to_the_caller_and_lists_every_text() {
        let mail = digest(&["one".into(), "two".into()]);
        assert!(matches!(mail.to, Recipients::Me));
        assert_eq!(mail.subject, "2 newest items");
        assert_eq!(mail.text, "- one\n- two\n");
        assert_eq!(digest(&[]).text, "No items yet.");
    }
    // jc:end

    // jc:service ai
    #[test]
    fn the_summary_asks_for_one_sentence_over_the_texts() {
        let request = summary_request(&["one".into(), "two".into()]);
        assert!(matches!(request.messages[0].role, Role::System));
        assert_eq!(request.messages[1].content, "one\ntwo");
        assert_eq!(request.max_tokens, Some(100));
    }
    // jc:end

    #[test]
    fn an_unknown_route_is_not_found() {
        let request = Request {
            method: "GET".into(),
            path: "/api/nothing".into(),
            ..Request::default()
        };
        assert_eq!(handle(request).status, 404);
    }
}
