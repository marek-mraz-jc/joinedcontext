//! The server half of news-topics (T-3352): what the browser cannot keep. The city's news feed
//! holds its recent articles and drops old ones, so the server reads the feed through the App's
//! own Endpoint with the caller's token, fits the browser's own topic model (`../wasm`) to each
//! ISO week's articles, and keeps every week's topics and their keywords in its own schema, with
//! the week's articles as a corpus file under the App's prefix. Choosing the period, the number
//! of topics and a search stays in the browser.
//!
//! | Route | What it does |
//! |---|---|
//! | `GET /api/weeks` | the topics of every kept week, the newest first; recomputed from the feed when older than six hours |
//! | `GET /api/weeks/{week}/corpus` | a URL to download the week's articles from (`2026-W41`) |
//!
//! Roles (T-3828): a signed-in reader follows topics of their own, each by its top keyword; a
//! steward names a topic by its top keyword and merges topics by giving them one name, which
//! everyone sees. Every write leaves a row in `app_audit`.
//!
//! | Route | Who | What it does |
//! |---|---|---|
//! | `GET /api/me` | anyone | the caller's id, name and roles, `null` when signed out |
//! | `GET /api/follows` | signed in | the caller's followed keywords, newest first |
//! | `POST /api/follows` `{term}` | signed in | a keyword followed; 409 when followed already or past 30 |
//! | `DELETE /api/follows/{id}` | owner | no longer followed |
//! | `GET /api/names` | anyone | every topic name a steward gave |
//! | `POST /api/names` `{term, name}` | steward | the topic of that top keyword named, or renamed |
//! | `DELETE /api/names/{id}` | steward | the name taken back |
//! | `GET /api/history` | signed in | the newest 100 changes; a viewer's own, a steward's every one |

use std::collections::BTreeMap;

use jc_app_sdk::access::Owned;
use jc_app_sdk::blob::{self, Method};
use jc_app_sdk::gateway;
use jc_app_sdk::http::{Params, Request, Response, Router};
use jc_app_sdk::identity::{self, Caller};
use jc_app_sdk::sql::{self, Value};
use news_topics_wasm::{iso_week, run_analyse, AnalysisOutput};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

/// How long a presigned URL lives; the host allows at most 300 seconds.
const URL_SECONDS: u32 = 120;
/// Articles read from the feed per page, and at most in all.
const PAGE: usize = 500;
const MOST: usize = 5000;
/// Topics per week, as the page's default; a week with fewer articles gets fewer.
const TOPICS: usize = 5;
/// Weeks answered at most: two years.
const WEEKS: i64 = 104;
const STEWARD: &str = "steward";
const FOLLOWS: Owned = Owned::new("follows", "");
const NAMES: Owned = Owned::new("topic_names", STEWARD);
/// Keywords a reader follows at most, and changes answered in History.
const MOST_FOLLOWS: i64 = 30;
const HISTORY: i64 = 100;

/// One article of a week's corpus, as the file keeps it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Article {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub published: String,
    pub url: String,
}

/// A keyValues text as a person reads it: a string, or the Finnish or English of a language map.
pub fn text_of(value: Option<&Json>) -> String {
    match value {
        Some(Json::String(text)) => text.clone(),
        Some(Json::Object(map)) => ["fi", "en", "@value"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Json::as_str))
            .or_else(|| map.values().find_map(Json::as_str))
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

/// The articles of each ISO week (`2026-W41`) of `entities` (keyValues Articles); an article
/// without a readable date is in no week.
pub fn by_week(entities: &[Json]) -> BTreeMap<String, Vec<Article>> {
    let mut weeks: BTreeMap<String, Vec<Article>> = BTreeMap::new();
    for entity in entities {
        let published = text_of(entity.get("datePublished"));
        let Some(week) = iso_week(&published) else {
            continue;
        };
        let id = entity["id"].as_str().unwrap_or_default().to_owned();
        let title = text_of(entity.get("name"));
        if id.is_empty() || title.trim().is_empty() {
            continue;
        }
        weeks.entry(week).or_default().push(Article {
            id,
            title,
            summary: text_of(entity.get("description")),
            published,
            url: text_of(entity.get("url")),
        });
    }
    weeks
}

/// One line of 1 to `most` characters, trimmed.
pub fn line(text: &str, most: usize, what: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > most || text.chars().any(char::is_control) {
        return Err(format!("{what} is one line of 1 to {most} characters"));
    }
    Ok(text.to_owned())
}

/// A keyword a reader follows.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewFollow {
    pub term: String,
}

/// A topic's name: its top keyword and what everyone reads instead of its keywords.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewName {
    pub term: String,
    pub name: String,
}

/// A keyword as kept: one line of at most 60 characters, in lower case as the model writes it.
pub fn checked_term(term: &str) -> Result<String, String> {
    line(term, 60, "a keyword").map(|term| term.to_lowercase())
}

/// The topic model of one week's articles, the largest topic first.
pub fn model(articles: &[Article]) -> Result<AnalysisOutput, String> {
    let input = json!({
        "articles": articles.iter().map(|a| json!({"id": a.id, "title": a.title, "summary": a.summary, "published": a.published})).collect::<Vec<_>>(),
        "k": TOPICS.min(articles.len().max(1)),
        "seed": 1,
    });
    let answer = run_analyse(&input.to_string())?;
    serde_json::from_str(&answer)
        .map_err(|err| format!("the topic model answered no topics: {err}"))
}

/// Every Article the App's Endpoint lets the caller read, page by page, up to [`MOST`].
fn articles() -> Result<Vec<Json>, gateway::Error> {
    let mut all = Vec::new();
    loop {
        let path = format!(
            "/ngsi-ld/v1/entities?type=Article&options=keyValues&limit={PAGE}&offset={}&attrs={}",
            all.len(),
            gateway::encode("name,description,url,datePublished"),
        );
        let page: Vec<Json> = gateway::get_json(&path)?;
        let last = page.len() < PAGE;
        all.extend(page);
        if last || all.len() >= MOST {
            all.truncate(MOST);
            return Ok(all);
        }
    }
}

fn corpus_key(week: &str) -> String {
    format!("corpus/{week}.json")
}

/// Reads the feed, fits each week's model and keeps its topics and its corpus; a week the feed no
/// longer holds keeps what was stored for it.
fn refresh() -> Result<(), Response> {
    let entities = articles().map_err(Response::from)?;
    for (week, articles) in by_week(&entities) {
        let output = model(&articles)
            .map_err(|why| Response::problem(500, "Internal Server Error", &why))?;
        let corpus = serde_json::to_vec(&articles).unwrap_or_default();
        blob::put(&corpus_key(&week), &corpus, Some("application/json"))
            .map_err(Response::from_blob)?;
        sql::execute(
            "insert into topic_runs (week, articles, computed_at) values ($1, $2, now()) \
             on conflict (week) do update set articles = excluded.articles, computed_at = now()",
            &[
                Value::from(week.clone()),
                Value::from(articles.len() as i64),
            ],
        )
        .map_err(Response::from_sql)?;
        sql::execute(
            "delete from week_topics where week = $1",
            &[Value::from(week.clone())],
        )
        .map_err(Response::from_sql)?;
        for topic in &output.topics {
            let keywords: Vec<Json> = topic
                .keywords
                .iter()
                .map(|k| json!({"term": k.term, "weight": k.weight}))
                .collect();
            sql::execute(
                "insert into week_topics (week, topic, share, articles, keywords) values ($1, $2, $3, $4, $5::jsonb)",
                &[
                    Value::from(week.clone()),
                    Value::from(topic.id as i64),
                    Value::from(f64::from(topic.share)),
                    Value::from(topic.articles.len() as i64),
                    Value::Json(Json::Array(keywords).to_string()),
                ],
            )
            .map_err(Response::from_sql)?;
        }
    }
    Ok(())
}

fn weeks(_: &Request, _: &Params) -> Response {
    let fresh = match sql::query(
        "select count(*) from topic_runs where computed_at > now() - interval '6 hours'",
        &[],
    ) {
        Ok(rows) => !matches!(
            rows.values.first().and_then(|row| row.first()),
            Some(Value::Int(0)) | None
        ),
        Err(err) => return Response::from_sql(err),
    };
    // A feed that cannot be read leaves what is stored readable, and says so.
    let problem = if fresh { None } else { refresh().err() };
    let runs = match sql::query(
        r#"select week, articles, to_char(computed_at at time zone 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as computed_at
           from topic_runs order by week desc limit $1"#,
        &[Value::from(WEEKS)],
    ) {
        Ok(rows) => sql::objects(&rows),
        Err(err) => return Response::from_sql(err),
    };
    if runs.is_empty() {
        if let Some(problem) = problem {
            return problem;
        }
    }
    let topics = match sql::query(
        "select week, topic, share::float8 as share, articles, keywords from week_topics \
         where week in (select week from topic_runs order by week desc limit $1) order by week desc, topic",
        &[Value::from(WEEKS)],
    ) {
        Ok(rows) => sql::objects(&rows),
        Err(err) => return Response::from_sql(err),
    };
    let mut by_week: BTreeMap<String, Vec<Json>> = BTreeMap::new();
    for mut topic in topics {
        let week = topic
            .remove("week")
            .and_then(|w| w.as_str().map(str::to_owned))
            .unwrap_or_default();
        by_week.entry(week).or_default().push(Json::Object(topic));
    }
    let weeks: Vec<Json> = runs
        .into_iter()
        .map(|mut run| {
            let week = run
                .get("week")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned();
            run.insert(
                "topics".into(),
                Json::Array(by_week.remove(&week).unwrap_or_default()),
            );
            Json::Object(run)
        })
        .collect();
    Response::json(200, &json!({ "weeks": weeks, "stale": problem.is_some() }))
}

/// A week as the routes name it: `YYYY-Www`.
pub fn is_week(week: &str) -> bool {
    let bytes = week.as_bytes();
    bytes.len() == 8
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && &week[4..6] == "-W"
        && bytes[6..].iter().all(u8::is_ascii_digit)
}

fn corpus(_: &Request, params: &Params) -> Response {
    let week = &params["week"];
    if !is_week(week) {
        return Response::problem(404, "Not Found", "a week is written 2026-W41");
    }
    match sql::query(
        "select 1 from topic_runs where week = $1",
        &[Value::from(week.as_str())],
    ) {
        Ok(rows) if rows.values.is_empty() => {
            Response::problem(404, "Not Found", "the server keeps no such week")
        }
        Ok(_) => match blob::presign(&corpus_key(week), Method::Get, URL_SECONDS) {
            Ok(url) => Response::json(200, &json!({ "url": url })),
            Err(err) => Response::from_blob(err),
        },
        Err(err) => Response::from_sql(err),
    }
}

fn me(_: &Request, _: &Params) -> Response {
    match identity::caller() {
        Some(caller) => Response::json(
            200,
            &json!({"id": caller.id, "name": caller.name, "roles": caller.roles}),
        ),
        None => Response::json(200, &Json::Null),
    }
}

fn bad(why: &str) -> Response {
    Response::problem(400, "Bad Request", why)
}

fn conflict(why: &str) -> Response {
    Response::problem(409, "Conflict", why)
}

/// The first column of the first row as a number, as `count(*)` answers.
fn count(statement: &str, params: &[Value]) -> Result<i64, Response> {
    match sql::query(statement, params) {
        Ok(rows) => match rows.values.first().and_then(|row| row.first()) {
            Some(Value::Int(n)) => Ok(*n),
            _ => Ok(0),
        },
        Err(err) => Err(Response::from_sql(err)),
    }
}

fn follows(_: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    match sql::query(
        "select id, term from follows where owner_id = $1 order by id desc limit $2",
        &[Value::from(caller.id.as_str()), Value::Int(MOST_FOLLOWS)],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

fn follow(request: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let new: NewFollow = match request.json() {
        Ok(new) => new,
        Err(answer) => return answer,
    };
    let term = match checked_term(&new.term) {
        Ok(term) => term,
        Err(why) => return bad(&why),
    };
    let owner = Value::from(caller.id.as_str());
    match count(
        "select count(*) from follows where owner_id = $1 and term = $2",
        &[owner.clone(), Value::from(term.as_str())],
    ) {
        Ok(0) => {}
        Ok(_) => return conflict("this keyword is followed already"),
        Err(answer) => return answer,
    }
    match count("select count(*) from follows where owner_id = $1", &[owner]) {
        Ok(n) if n >= MOST_FOLLOWS => {
            return conflict(&format!(
                "at most {MOST_FOLLOWS} keywords are followed; stop following one first"
            ))
        }
        Ok(_) => {}
        Err(answer) => return answer,
    }
    match FOLLOWS.insert(
        &caller,
        &["term"],
        vec![Value::from(term.as_str())],
        &format!("followed {term}"),
    ) {
        Ok(rows) => {
            let row = sql::objects(&rows).into_iter().next().unwrap_or_default();
            Response::json(201, &json!({"id": row.get("id"), "term": term}))
        }
        Err(answer) => answer,
    }
}

fn unfollow(_: &Request, params: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let Ok(id) = params["id"].parse() else {
        return Response::problem(404, "Not Found", "no such follow");
    };
    match FOLLOWS.delete(&caller, id, "stopped following a keyword") {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

fn names(_: &Request, _: &Params) -> Response {
    match sql::query(
        r#"select id, term, name, owner_name as "ownerName" from topic_names order by term"#,
        &[],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

fn steward() -> Result<Caller, Response> {
    let caller = Caller::require()?;
    caller.require_role(&[STEWARD])?;
    Ok(caller)
}

fn name(request: &Request, _: &Params) -> Response {
    // The role is checked before the body is read, so nothing in the body stands in for it.
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let new: NewName = match request.json() {
        Ok(new) => new,
        Err(answer) => return answer,
    };
    let (term, name) = match checked_term(&new.term)
        .and_then(|term| Ok((term, line(&new.name, 80, "a topic's name")?)))
    {
        Ok(checked) => checked,
        Err(why) => return bad(&why),
    };
    let existing = match sql::query(
        "select id from topic_names where term = $1",
        &[Value::from(term.as_str())],
    ) {
        Ok(rows) => rows.values.first().and_then(|row| match row.first() {
            Some(Value::Int(id)) => Some(*id),
            _ => None,
        }),
        Err(err) => return Response::from_sql(err),
    };
    let summary = format!("named the topic of {term} {name}");
    let id = match existing {
        Some(id) => match NAMES.update(
            &caller,
            id,
            &["name", "owner_name"],
            vec![
                Value::from(name.as_str()),
                Value::from(caller.name.as_str()),
            ],
            &summary,
        ) {
            Ok(()) => Json::from(id),
            Err(answer) => return answer,
        },
        None => match NAMES.insert(
            &caller,
            &["term", "name", "owner_name"],
            vec![
                Value::from(term.as_str()),
                Value::from(name.as_str()),
                Value::from(caller.name.as_str()),
            ],
            &summary,
        ) {
            Ok(rows) => sql::objects(&rows)
                .into_iter()
                .next()
                .and_then(|mut row| row.remove("id"))
                .unwrap_or_default(),
            Err(answer) => return answer,
        },
    };
    Response::json(
        if existing.is_some() { 200 } else { 201 },
        &json!({"id": id, "term": term, "name": name, "ownerName": caller.name}),
    )
}

fn unname(_: &Request, params: &Params) -> Response {
    let caller = match steward() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    let Ok(id) = params["id"].parse() else {
        return Response::problem(404, "Not Found", "no such name");
    };
    match NAMES.delete(&caller, id, "took back a topic's name") {
        Ok(()) => Response::no_content(),
        Err(answer) => answer,
    }
}

/// The newest changes: a steward's every one, anyone else's their own.
fn history(_: &Request, _: &Params) -> Response {
    let caller = match Caller::require() {
        Ok(caller) => caller,
        Err(answer) => return answer,
    };
    match sql::query(
        "select id, at, actor_name as \"actorName\", action, target, summary from app_audit \
         where actor_id = $1 or $2::boolean order by at desc, id desc limit $3",
        &[
            Value::from(caller.id.as_str()),
            Value::Boolean(caller.has_role(STEWARD)),
            Value::Int(HISTORY),
        ],
    ) {
        Ok(rows) => Response::json(200, &sql::objects(&rows)),
        Err(err) => Response::from_sql(err),
    }
}

pub fn handle(request: Request) -> Response {
    Router::new()
        .get("/api/me", me)
        .get("/api/follows", follows)
        .post("/api/follows", follow)
        .delete("/api/follows/{id}", unfollow)
        .get("/api/names", names)
        .post("/api/names", name)
        .delete("/api/names/{id}", unname)
        .get("/api/history", history)
        .get("/api/weeks", weeks)
        .get("/api/weeks/{week}/corpus", corpus)
        .handle(&request)
}

jc_app_sdk::app!(handle);

#[cfg(test)]
mod tests {
    use super::*;

    fn article(id: &str, name: Json, published: Json) -> Json {
        json!({"id": id, "type": "Article", "name": name, "description": "Kaupunki tiedottaa", "datePublished": published, "url": "https://www.hel.fi/x"})
    }

    #[test]
    fn a_text_is_a_string_a_language_map_or_a_date_value() {
        assert_eq!(text_of(Some(&json!("Uutinen"))), "Uutinen");
        assert_eq!(
            text_of(Some(&json!({"en": "News", "fi": "Uutinen"}))),
            "Uutinen"
        );
        assert_eq!(
            text_of(Some(
                &json!({"@type": "DateTime", "@value": "2026-10-08T07:00:00Z"})
            )),
            "2026-10-08T07:00:00Z"
        );
        assert_eq!(text_of(Some(&json!(3))), "");
        assert_eq!(text_of(None), "");
    }

    #[test]
    fn articles_fall_into_their_iso_week_and_one_without_a_date_or_title_into_none() {
        let weeks = by_week(&[
            article(
                "a",
                json!("Raitiotie Kalasatamaan"),
                json!("2026-10-05T08:00:00Z"),
            ),
            article(
                "b",
                json!({"fi": "Kirjasto avautuu"}),
                json!({"@type": "DateTime", "@value": "2026-10-11T20:00:00Z"}),
            ),
            article("c", json!("Uusi viikko"), json!("2026-10-12T06:00:00Z")),
            article("d", json!("Ei päivää"), json!("eilen")),
            article("e", json!("  "), json!("2026-10-12T06:00:00Z")),
            // Thursday 2026-01-01 is in ISO week 2026-W01.
            article("f", json!("Uusi vuosi"), json!("2026-01-01T10:00:00Z")),
        ]);
        assert_eq!(
            weeks.keys().collect::<Vec<_>>(),
            ["2026-W01", "2026-W41", "2026-W42"]
        );
        assert_eq!(
            weeks["2026-W41"]
                .iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(weeks["2026-W41"][1].title, "Kirjasto avautuu");
        assert_eq!(weeks["2026-W42"].len(), 1);
        assert!(by_week(&[]).is_empty());
    }

    #[test]
    fn a_weeks_model_has_at_most_five_topics_and_one_article_gets_one() {
        let articles: Vec<Article> = (0..12)
            .map(|i| Article {
                id: format!("n{i}"),
                title: if i % 2 == 0 {
                    format!("Raitiotie liikenne ratikka {i}")
                } else {
                    format!("Kirjasto lukeminen kirja {i}")
                },
                summary: String::new(),
                published: "2026-10-05T08:00:00Z".into(),
                url: String::new(),
            })
            .collect();
        let output = model(&articles).unwrap();
        assert!(!output.topics.is_empty() && output.topics.len() <= TOPICS);
        let shares: f32 = output.topics.iter().map(|t| t.share).sum();
        assert!(
            (shares - 1.0).abs() < 0.01 || !output.unassigned.is_empty(),
            "{shares}"
        );
        let one = model(&articles[..1]).unwrap();
        assert_eq!(one.topics.len(), 1);
        assert_eq!(one.topics[0].articles, ["n0"]);
    }

    #[test]
    fn a_week_is_written_like_2026_w41() {
        assert!(is_week("2026-W41"));
        for bad in ["2026-41", "2026-W4", "2026-W411", "../W41", "2026-w41", ""] {
            assert!(!is_week(bad), "{bad}");
        }
    }

    #[test]
    fn a_route_outside_the_api_or_a_bad_week_is_answered_in_words() {
        let get = |path: &str| {
            handle(Request {
                method: "GET".into(),
                path: path.into(),
                ..Request::default()
            })
        };
        assert_eq!(get("/api/nothing").status, 404);
        let bad = get("/api/weeks/..%2F..%2Fx/corpus");
        assert_eq!(bad.status, 404);
        assert!(String::from_utf8_lossy(&bad.body).contains("2026-W41"));
        let wrong = handle(Request {
            method: "POST".into(),
            path: "/api/weeks".into(),
            ..Request::default()
        });
        assert_eq!(wrong.status, 405);
    }

    #[test]
    fn a_keyword_and_a_name_are_one_trimmed_line() {
        assert_eq!(checked_term("  Kirjasto "), Ok("kirjasto".into()));
        assert!(checked_term("").is_err());
        assert!(checked_term("a\nb").is_err());
        assert!(checked_term(&"x".repeat(61)).is_err());
        assert_eq!(
            checked_term(&"ä".repeat(60)).map(|t| t.chars().count()),
            Ok(60)
        );
        assert_eq!(line(" Libraries ", 80, "a name"), Ok("Libraries".into()));
        assert_eq!(
            line(" ", 80, "a topic's name"),
            Err("a topic's name is one line of 1 to 80 characters".into())
        );
        assert!(serde_json::from_str::<NewName>(r#"{"term":"a","name":"b","owner":"x"}"#).is_err());
        assert!(serde_json::from_str::<NewFollow>(r#"{"term":"a","id":1}"#).is_err());
    }
}
