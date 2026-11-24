//! The NGSI-LD query card of an Endpoint (T-4759, T-4760): how to query it, its types and their
//! attributes as compact lines with units and allowed values, generated from the LinkML the
//! Endpoint serves the caller. The one generator: the gateway serves it as `card://{slug}`, the
//! Portal's Endpoint page shows that text, and the assistant gives it to the model.

use std::collections::BTreeMap;

use crate::display_order::{query_attributes, QueryAttribute};

/// Above this many tokens a card lists its types with their attribute counts, and a type's lines
/// come with the first query on it (the BBSK catalogue: 24 types, 1,317 attributes, ~6.2k tokens).
pub const CARD_TOKENS: usize = 1_500;

/// Tokens of a text, about characters / 3.7 (the ratio of the board's measurements).
pub fn tokens(text: &str) -> usize {
    text.chars().count() * 10 / 37
}

/// The card: the fixed NGSI-LD lines naming only the read tools `offers` grants, then the
/// attributes every type shares once, then each type's lines, or with `brief` each type's count.
/// `endpoint` is the Endpoint's `{project}/{name}`, `title` its title.
pub fn card(
    endpoint: &str,
    title: Option<&str>,
    offers: impl Fn(&str) -> bool,
    classes: &BTreeMap<String, Vec<QueryAttribute>>,
    brief: bool,
) -> String {
    let title = title.map(|t| format!(" ({t})")).unwrap_or_default();
    let reads: Vec<&str> = [
        ("query_entities", "query_entities (type, q, georel, pick)"),
        ("query_temporal", "query_temporal (history)"),
        ("summarize", "summarize (aggregates)"),
    ]
    .into_iter()
    .filter(|(tool, _)| offers(tool))
    .map(|(_, said)| said)
    .collect();
    let mut card = format!("NGSI-LD Endpoint {endpoint}{title}, ETSI GS CIM 009 / TS 104 175.");
    if !reads.is_empty() {
        card.push_str(&format!(" Read it with {}.", reads.join(", ")));
    }
    if offers("query_entities") || offers("query_temporal") {
        card.push_str("\nq: attr==value; AND, | OR, == != > >= < <= ~= (regex), ranges a..b; a text value in quotes; a Relationship by its target id (refParentArea==\"urn:ngsi-ld:...\").");
    }
    if offers("query_temporal") {
        // the broker's defaults and ceilings (ANTARES_TEMPORAL_DEFAULT_INSTANCES,
        // ANTARES_TEMPORAL_INSTANCE_LIMIT, ANTARES_DEFAULT_LIMIT, ANTARES_MAX_LIMIT; T-4800, T-4804)
        card.push_str("\nHistory: lastN and limit 100 by default, 1,000 at most; over a day hourly averages; more.next continues.");
    }
    if offers("query_entities") {
        card.push_str("\nGeo: georel near;maxDistance==<metres> or within on location. Units: compare in the unit shown (pm10 is ug/m3; a distance asked in km is sent in m).");
    }
    let shared = shared(classes);
    if !shared.is_empty() {
        card.push_str(&format!("\nEvery type also has: {}.", shared.join(", ")));
    }
    for (kind, attributes) in classes {
        if brief {
            card.push_str(&format!("\n{kind}: {} attributes", attributes.len()));
        } else {
            card.push_str(&type_lines(kind, attributes, &shared));
        }
    }
    if brief {
        card.push_str("\nA type's attributes come with your first query on it.");
    }
    card
}

/// The card an Endpoint serves for its LinkML `model`: whole, or brief past [`CARD_TOKENS`]. A
/// model that does not parse gives the query rules alone.
pub fn served(
    endpoint: &str,
    title: Option<&str>,
    offers: impl Fn(&str) -> bool,
    model: Option<&str>,
) -> String {
    let classes = model
        .and_then(|m| query_attributes(m).ok())
        .unwrap_or_default();
    let whole = card(endpoint, title, &offers, &classes, false);
    if tokens(&whole) <= CARD_TOKENS {
        whole
    } else {
        card(endpoint, title, &offers, &classes, true)
    }
}

/// The attribute lines every type writes alike, when there are two types or more.
fn shared(classes: &BTreeMap<String, Vec<QueryAttribute>>) -> Vec<String> {
    let mut all = classes.values();
    let (Some(first), true) = (all.next(), classes.len() > 1) else {
        return Vec::new();
    };
    first
        .iter()
        .map(|a| a.written.clone())
        .filter(|w| classes.values().all(|c| c.iter().any(|a| &a.written == w)))
        .collect()
}

/// One type's line, without the `shared` attributes, and its example `q` when it has one.
pub fn type_lines(kind: &str, attributes: &[QueryAttribute], shared: &[String]) -> String {
    let own: Vec<&str> = attributes
        .iter()
        .map(|a| a.written.as_str())
        .filter(|w| !shared.iter().any(|s| s == w))
        .collect();
    let mut lines = format!("\n{kind}: {}", own.join(", "));
    if let Some(example) = attributes.iter().find_map(|a| a.example.as_deref()) {
        lines.push_str(&format!("\n  e.g. q={example}"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr(slot: &str, written: &str, example: Option<&str>) -> QueryAttribute {
        QueryAttribute {
            slot: slot.to_owned(),
            written: written.to_owned(),
            example: example.map(str::to_owned),
        }
    }

    fn two_types() -> BTreeMap<String, Vec<QueryAttribute>> {
        BTreeMap::from([
            (
                "AirQualityObserved".to_owned(),
                vec![
                    attr("name", "name", None),
                    attr("pm10", "pm10:float[ug/m3]", Some("pm10>0")),
                ],
            ),
            (
                "WeatherObserved".to_owned(),
                vec![attr("name", "name", None), attr("t", "t:float", None)],
            ),
        ])
    }

    #[test]
    fn tokens_count_characters_not_bytes() {
        assert_eq!(tokens(""), 0);
        assert_eq!(tokens(&"a".repeat(37)), 10);
        assert_eq!(tokens(&"č".repeat(37)), 10, "a multibyte character is one");
    }

    #[test]
    fn a_card_offering_every_read_names_each_tool_and_its_rules() {
        let card = card("p/air", Some("Air"), |_| true, &two_types(), false);
        assert!(card.starts_with("NGSI-LD Endpoint p/air (Air), ETSI GS CIM 009"));
        assert!(card.contains(
            "Read it with query_entities (type, q, georel, pick), query_temporal (history), summarize (aggregates)."
        ));
        assert!(card.contains("\nq: attr==value"));
        assert!(card.contains("\nHistory: lastN"));
        assert!(card.contains("\nGeo: georel"));
        assert!(card.contains("\nEvery type also has: name."));
        assert!(card.contains("\nAirQualityObserved: pm10:float[ug/m3]\n  e.g. q=pm10>0"));
        assert!(card.contains("\nWeatherObserved: t:float"));
        assert!(!card.contains("come with your first query"));
    }

    #[test]
    fn a_card_without_reads_or_title_holds_only_the_endpoint_line_and_types() {
        let card = card("p/air", None, |_| false, &BTreeMap::new(), false);
        assert_eq!(
            card,
            "NGSI-LD Endpoint p/air, ETSI GS CIM 009 / TS 104 175."
        );
    }

    #[test]
    fn summarize_alone_names_no_query_rules() {
        let card = card("p/a", None, |t| t == "summarize", &BTreeMap::new(), false);
        assert!(card.contains("Read it with summarize (aggregates)."));
        assert!(!card.contains("\nq:") && !card.contains("History") && !card.contains("Geo"));
    }

    #[test]
    fn history_alone_gives_the_q_rules_and_the_history_line_without_geo() {
        let card = card(
            "p/a",
            None,
            |t| t == "query_temporal",
            &BTreeMap::new(),
            false,
        );
        assert!(card.contains("\nq: attr==value") && card.contains("\nHistory:"));
        assert!(!card.contains("\nGeo:"));
    }

    #[test]
    fn a_brief_card_counts_each_types_attributes() {
        let card = card("p/air", None, |_| true, &two_types(), true);
        assert!(card.contains("\nAirQualityObserved: 2 attributes"));
        assert!(card.contains("\nWeatherObserved: 2 attributes"));
        assert!(card.ends_with("\nA type's attributes come with your first query on it."));
        assert!(!card.contains("e.g. q="));
    }

    #[test]
    fn one_type_shares_nothing() {
        let mut one = two_types();
        one.remove("WeatherObserved");
        assert!(shared(&one).is_empty());
        assert!(shared(&BTreeMap::new()).is_empty());
        assert_eq!(shared(&two_types()), ["name"]);
    }

    #[test]
    fn type_lines_leave_out_the_shared_attributes_and_name_the_first_example() {
        let attributes = [
            attr("a", "a", None),
            attr("b", "b:a|b", Some("b==\"a\"")),
            attr("c", "c:float", Some("c>0")),
        ];
        assert_eq!(
            type_lines("T", &attributes, &["a".to_owned()]),
            "\nT: b:a|b, c:float\n  e.g. q=b==\"a\""
        );
        assert_eq!(type_lines("T", &[], &[]), "\nT: ");
    }

    const MODEL: &str = r"
id: https://example.org/air
name: air
default_range: string
slots:
  name: {}
  pm10: { range: float }
classes:
  AirQualityObserved:
    slots: [name, pm10]
";

    #[test]
    fn served_without_a_model_or_with_a_broken_one_gives_the_rules_alone() {
        let none = served("p/a", None, |_| true, None);
        assert!(none.contains("\nq: attr==value") && !none.contains("AirQualityObserved"));
        assert_eq!(served("p/a", None, |_| true, Some("classes: [")), none);
    }

    #[test]
    fn served_gives_the_whole_card_of_a_small_model() {
        let card = served("p/a", None, |_| true, Some(MODEL));
        assert!(card.contains("\nAirQualityObserved: "), "{card}");
        assert!(!card.contains("attributes come with"), "{card}");
    }

    #[test]
    fn served_turns_brief_past_the_token_budget() {
        let mut model =
            String::from("id: https://example.org/big\nname: big\ndefault_range: string\nslots:\n");
        for i in 0..400 {
            model.push_str(&format!("  attribute_with_a_long_name_{i}: {{}}\n"));
        }
        model.push_str("classes:\n  Big:\n    slots: [");
        model.push_str(
            &(0..400)
                .map(|i| format!("attribute_with_a_long_name_{i}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        model.push_str("]\n");
        let card = served("p/a", None, |_| true, Some(&model));
        assert!(card.contains("\nBig: 400 attributes"), "{card}");
        assert!(tokens(&card) <= CARD_TOKENS);
    }
}
