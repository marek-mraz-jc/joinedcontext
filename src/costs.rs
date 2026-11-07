//! What a month of the installation costs each project (T-5126, API/01 §29, API/05 §3.1): the
//! operator's price table (`JC_PORTAL_COST_PRICES_FILE`) split by a rule an administrator can
//! check by hand.
//!
//! Neither dev nor prod runs Prometheus, so there is no CPU or memory history per pod to read.
//! On this platform the broker and its Postgres carry the load and their work grows with what a
//! space stores, so the node's monthly price (with the fixed items: addresses, backups) is split
//! by each project's share of the bytes the brokers measure for its spaces. The volumes are
//! billed per gigabyte provisioned: a project pays its own bytes at the blended volume price and
//! the unused rest stays with the platform. AI is the assistant's priced turns of the month per
//! AssistantDeployment, converted at the table's rate. Every amount not given to a project is
//! the platform's row, so the projects and the platform add up to the table, which is what the
//! Hetzner invoice bills (`scripts/cost-reconcile.py` checks the two).
//! ponytail: storage is measured now, not averaged over the month; a daily sample table when a
//! project's size swings within a month.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A GiB: Hetzner sizes volumes in GiB.
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// One priced line of the table: a node, an address, a backup.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PricedItem {
    /// What it is, e.g. `cpx42 nbg1`.
    pub name: String,
    /// Its price a month, in the table's currency, without VAT.
    pub monthly: f64,
}

/// One volume, billed per GiB provisioned.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PricedVolume {
    pub name: String,
    pub gigabytes: f64,
    /// The price of one GiB a month.
    pub monthly_per_gigabyte: f64,
}

/// The operator's price table: what the cloud bills a month.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CostPrices {
    /// ISO 4217, e.g. `EUR`.
    pub currency: String,
    /// The table's currency for one US dollar, which the model provider bills in.
    pub per_usd: f64,
    pub nodes: Vec<PricedItem>,
    #[serde(default)]
    pub volumes: Vec<PricedVolume>,
    /// Billed every month whatever runs: addresses, backups.
    #[serde(default)]
    pub fixed: Vec<PricedItem>,
}

impl CostPrices {
    /// Why the table cannot price anything, naming the field; `None` when it can.
    pub fn refusal(&self) -> Option<String> {
        let price = |v: f64| v.is_finite() && v >= 0.0;
        if self.currency.len() != 3 || !self.currency.bytes().all(|b| b.is_ascii_uppercase()) {
            return Some(format!(
                "currency '{}' is no ISO 4217 code such as EUR",
                self.currency
            ));
        }
        if !price(self.per_usd) || self.per_usd == 0.0 {
            return Some("perUsd is the table's currency for one US dollar, e.g. 0.92".into());
        }
        if self.nodes.is_empty() {
            return Some("nodes lists no node: name each server with its monthly price".into());
        }
        let items = self.nodes.iter().chain(&self.fixed);
        if let Some(bad) = items
            .into_iter()
            .find(|i| i.name.is_empty() || !price(i.monthly))
        {
            return Some(format!("'{}': monthly is a price of 0 or more", bad.name));
        }
        let volume = |v: &&PricedVolume| {
            v.name.is_empty()
                || !(v.gigabytes.is_finite() && v.gigabytes > 0.0)
                || !price(v.monthly_per_gigabyte)
        };
        if let Some(bad) = self.volumes.iter().find(volume) {
            return Some(format!(
                "volume '{}': gigabytes is above 0 and monthlyPerGigabyte 0 or more",
                bad.name
            ));
        }
        None
    }

    /// The nodes and the fixed items a month.
    pub fn node_total(&self) -> f64 {
        self.nodes
            .iter()
            .chain(&self.fixed)
            .map(|i| i.monthly)
            .sum()
    }

    /// The volumes a month.
    pub fn volume_total(&self) -> f64 {
        self.volumes
            .iter()
            .map(|v| v.gigabytes * v.monthly_per_gigabyte)
            .sum()
    }

    fn volume_bytes(&self) -> f64 {
        self.volumes.iter().map(|v| v.gigabytes).sum::<f64>() * GIB
    }
}

/// The calendar month `text` names (`2026-09`), as its first instant and the next month's; or
/// why not. A month that has not started is refused: it has no cost yet.
pub fn month(text: &str, now: DateTime<Utc>) -> Result<(DateTime<Utc>, DateTime<Utc>), String> {
    let first = (text.len() == 7)
        .then(|| NaiveDate::parse_from_str(&format!("{text}-01"), "%Y-%m-%d").ok())
        .flatten()
        .ok_or_else(|| format!("month '{text}' is no month such as 2026-09"))?;
    let start = Utc.from_utc_datetime(&first.and_time(chrono::NaiveTime::MIN));
    if start > now {
        return Err(format!("month '{text}' has not started yet"));
    }
    let next = first
        .checked_add_months(chrono::Months::new(1))
        .ok_or_else(|| format!("month '{text}' is out of range"))?;
    Ok((
        start,
        Utc.from_utc_datetime(&next.and_time(chrono::NaiveTime::MIN)),
    ))
}

/// The month before `now`'s, as `2026-09`: the last one billed whole.
pub fn last_month(now: DateTime<Utc>) -> String {
    let (year, month) = match now.month() {
        1 => (now.year() - 1, 12),
        m => (now.year(), m - 1),
    };
    format!("{year:04}-{month:02}")
}

/// One space's storage as measured, or why it is missing.
#[derive(Debug, Clone, PartialEq)]
pub struct SpaceMeasure {
    pub space: String,
    pub bytes: Result<u64, String>,
}

/// One assistant's month as the assistant counted it, or why it is missing.
#[derive(Debug, Clone, PartialEq)]
pub struct AssistantMeasure {
    pub assistant: String,
    pub spend: Result<AiSpend, String>,
}

/// An assistant's priced turns of the month.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AiSpend {
    pub cost_usd: f64,
    pub questions: u64,
    /// Turns the provider reported no price for, counted apart and never as 0.
    pub uncosted_questions: u64,
}

/// One space's line.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCost {
    pub space: String,
    /// The entities and history the broker measured; absent when it did not answer.
    pub bytes: Option<u64>,
    pub storage: f64,
    /// Why `bytes` is missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One assistant's line.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssistantCost {
    pub assistant: String,
    pub questions: u64,
    pub uncosted_questions: u64,
    /// What the provider billed, in USD; absent when the assistant did not answer.
    pub cost_usd: Option<f64>,
    pub cost: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One project's month, in the table's currency.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCost {
    pub project: String,
    /// The project's share of the nodes and fixed items, by its share of the measured bytes.
    pub node: f64,
    /// Its bytes at the blended volume price.
    pub storage: f64,
    pub ai: f64,
    pub total: f64,
    /// Its share of the measured bytes, 0 to 1.
    pub share: f64,
    pub spaces: Vec<SpaceCost>,
    pub assistants: Vec<AssistantCost>,
}

/// What no project carries: the volumes' unused gigabytes, and the nodes when nothing was
/// measured.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCost {
    pub node: f64,
    pub storage: f64,
    pub total: f64,
}

/// The month split.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Split {
    pub projects: Vec<ProjectCost>,
    pub platform: PlatformCost,
    /// The table: what the cloud bills, which the projects and the platform add up to.
    pub infrastructure: f64,
    pub ai: f64,
    pub total: f64,
}

/// Cents: an amount as the invoice states it.
fn cents(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Splits the month by the module's rule. A space or an assistant that did not answer counts 0
/// and keeps its reason, so the page says the split is partial instead of failing.
pub fn split(
    prices: &CostPrices,
    measured: BTreeMap<String, (Vec<SpaceMeasure>, Vec<AssistantMeasure>)>,
) -> Split {
    let bytes_of = |spaces: &[SpaceMeasure]| -> u64 {
        spaces.iter().filter_map(|s| s.bytes.as_ref().ok()).sum()
    };
    let all: u64 = measured.values().map(|(spaces, _)| bytes_of(spaces)).sum();
    let node_total = prices.node_total();
    let volume_total = prices.volume_total();
    let volume_bytes = prices.volume_bytes();
    // More bytes than provisioned (a disk the table omits) shares the volumes by bytes instead.
    let whole = volume_bytes.max(all as f64);
    let per_byte = if whole > 0.0 {
        volume_total / whole
    } else {
        0.0
    };
    let projects: Vec<ProjectCost> = measured
        .into_iter()
        .map(|(project, (spaces, assistants))| {
            let own = bytes_of(&spaces);
            let share = if all == 0 {
                0.0
            } else {
                own as f64 / all as f64
            };
            let spaces: Vec<SpaceCost> = spaces
                .into_iter()
                .map(|s| SpaceCost {
                    space: s.space,
                    bytes: s.bytes.as_ref().ok().copied(),
                    storage: cents(*s.bytes.as_ref().unwrap_or(&0) as f64 * per_byte),
                    reason: s.bytes.err(),
                })
                .collect();
            let assistants: Vec<AssistantCost> = assistants
                .into_iter()
                .map(|a| {
                    let spend = a.spend.as_ref().ok().copied().unwrap_or_default();
                    AssistantCost {
                        assistant: a.assistant,
                        questions: spend.questions,
                        uncosted_questions: spend.uncosted_questions,
                        cost_usd: a.spend.is_ok().then_some(spend.cost_usd),
                        cost: cents(spend.cost_usd * prices.per_usd),
                        reason: a.spend.err(),
                    }
                })
                .collect();
            let node = cents(node_total * share);
            let storage = cents(own as f64 * per_byte);
            let ai = cents(assistants.iter().map(|a| a.cost).sum());
            ProjectCost {
                project,
                node,
                storage,
                ai,
                total: cents(node + storage + ai),
                share,
                spaces,
                assistants,
            }
        })
        .collect();
    let given = |part: fn(&ProjectCost) -> f64| projects.iter().map(part).sum::<f64>();
    let node = cents((node_total - given(|p| p.node)).max(0.0));
    let storage = cents((volume_total - given(|p| p.storage)).max(0.0));
    let ai = cents(given(|p| p.ai));
    let infrastructure = cents(node_total + volume_total);
    Split {
        platform: PlatformCost {
            node,
            storage,
            total: cents(node + storage),
        },
        projects,
        infrastructure,
        ai,
        total: cents(infrastructure + ai),
    }
}

/// The CSV columns, in order.
pub const CSV_COLUMNS: [&str; 7] = [
    "month", "project", "part", "item", "bytes", "cost", "currency",
];

/// One CSV cell: quoted when it holds a separator, a quote or a line break, and a leading
/// formula sign defused, so a spreadsheet never runs a name as a formula.
fn cell(text: &str) -> String {
    let text = match text.chars().next() {
        Some('=' | '+' | '-' | '@' | '\t' | '\r') => format!("'{text}"),
        _ => text.to_owned(),
    };
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

/// The split as CSV: one line per space, assistant and node share, the platform's lines last.
pub fn csv(month: &str, currency: &str, split: &Split) -> String {
    let mut out = CSV_COLUMNS.join(",") + "\n";
    let mut line = |project: &str, part: &str, item: &str, bytes: Option<u64>, cost: f64| {
        let bytes = bytes.map(|b| b.to_string()).unwrap_or_default();
        let cells = [
            month,
            project,
            part,
            item,
            &bytes,
            &format!("{cost:.2}"),
            currency,
        ];
        out += &cells.map(cell).join(",");
        out.push('\n');
    };
    for p in &split.projects {
        line(&p.project, "node", "", None, p.node);
        for s in &p.spaces {
            line(&p.project, "storage", &s.space, s.bytes, s.storage);
        }
        for a in &p.assistants {
            line(&p.project, "ai", &a.assistant, None, a.cost);
        }
    }
    line("", "platform-node", "", None, split.platform.node);
    line("", "platform-storage", "", None, split.platform.storage);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prices() -> CostPrices {
        serde_norway::from_str(
            "currency: EUR\nperUsd: 0.5\nnodes: [{name: cpx42, monthly: 90}]\n\
             fixed: [{name: ipv4, monthly: 10}]\n\
             volumes: [{name: pg, gigabytes: 10, monthlyPerGigabyte: 2}]\n",
        )
        .expect("the table parses")
    }

    fn space(name: &str, bytes: Result<u64, String>) -> SpaceMeasure {
        SpaceMeasure {
            space: name.into(),
            bytes,
        }
    }

    fn assistant(name: &str, spend: Result<AiSpend, String>) -> AssistantMeasure {
        AssistantMeasure {
            assistant: name.into(),
            spend,
        }
    }

    /// T-5126: the node goes by bytes share, the volume by own bytes, AI at the rate; the
    /// projects and the platform add up to the table.
    #[test]
    fn the_projects_and_the_platform_add_up_to_the_table() {
        let gib = GIB as u64;
        let ai = AiSpend {
            cost_usd: 3.0,
            questions: 40,
            uncosted_questions: 2,
        };
        let measured = BTreeMap::from([
            (
                "a".to_owned(),
                (
                    vec![space("s1", Ok(3 * gib)), space("s2", Ok(gib))],
                    vec![assistant("help", Ok(ai))],
                ),
            ),
            ("b".to_owned(), (vec![space("s3", Ok(4 * gib))], vec![])),
        ]);
        let split = split(&prices(), measured);
        let (a, b) = (&split.projects[0], &split.projects[1]);
        assert_eq!((a.node, a.storage, a.ai, a.total), (50.0, 8.0, 1.5, 59.5));
        assert_eq!((b.node, b.storage, b.ai, b.total), (50.0, 8.0, 0.0, 58.0));
        assert_eq!(a.spaces[0].storage, 6.0);
        assert_eq!(a.assistants[0].cost_usd, Some(3.0));
        assert_eq!(
            split.platform,
            PlatformCost {
                node: 0.0,
                storage: 4.0,
                total: 4.0
            }
        );
        assert_eq!(
            (split.infrastructure, split.ai, split.total),
            (120.0, 1.5, 121.5)
        );
        let sum: f64 = split.projects.iter().map(|p| p.node + p.storage).sum();
        assert_eq!(cents(sum + split.platform.total), split.infrastructure);
    }

    /// Nothing measured: the whole table is the platform's, and a missing answer keeps its
    /// reason instead of failing the month.
    #[test]
    fn a_missing_answer_counts_zero_and_says_why() {
        let measured = BTreeMap::from([(
            "a".to_owned(),
            (
                vec![space("s1", Err("broker down".into()))],
                vec![assistant("help", Err("assistant down".into()))],
            ),
        )]);
        let split = split(&prices(), measured);
        let a = &split.projects[0];
        assert_eq!((a.node, a.storage, a.ai, a.share), (0.0, 0.0, 0.0, 0.0));
        assert_eq!(a.spaces[0].reason.as_deref(), Some("broker down"));
        assert_eq!(a.spaces[0].bytes, None);
        assert_eq!(a.assistants[0].cost_usd, None);
        assert_eq!(split.platform.total, 120.0);
    }

    /// More bytes than the volumes hold shares the volumes by bytes, never above the table.
    #[test]
    fn bytes_past_the_volumes_never_bill_more_than_the_volumes() {
        let measured = BTreeMap::from([(
            "a".to_owned(),
            (vec![space("s", Ok(40 * GIB as u64))], vec![]),
        )]);
        let split = split(&prices(), measured);
        assert_eq!(split.projects[0].storage, 20.0);
        assert_eq!(split.platform.storage, 0.0);
    }

    #[test]
    fn a_table_that_cannot_price_is_refused_naming_the_field() {
        let mut bad = prices();
        bad.currency = "eur".into();
        assert!(bad.refusal().is_some_and(|r| r.contains("currency")));
        let mut bad = prices();
        bad.nodes[0].monthly = -1.0;
        assert!(bad.refusal().is_some_and(|r| r.contains("cpx42")));
        let mut bad = prices();
        bad.volumes[0].gigabytes = 0.0;
        assert!(bad.refusal().is_some_and(|r| r.contains("pg")));
        let mut bad = prices();
        bad.per_usd = f64::NAN;
        assert!(bad.refusal().is_some_and(|r| r.contains("perUsd")));
        assert_eq!(prices().refusal(), None);
        let unknown =
            serde_norway::from_str::<CostPrices>("currency: EUR\nperUsd: 1\nnodes: []\nvat: 20\n");
        assert!(unknown.is_err());
    }

    #[test]
    fn a_month_is_a_started_calendar_month() {
        let now = Utc.with_ymd_and_hms(2026, 10, 22, 1, 0, 0).unwrap();
        let (from, to) = month("2026-09", now).expect("September");
        assert_eq!(from.to_rfc3339(), "2026-09-01T00:00:00+00:00");
        assert_eq!(to.to_rfc3339(), "2026-10-01T00:00:00+00:00");
        assert_eq!(
            month("2025-12", now).unwrap().1.to_rfc3339(),
            "2026-01-01T00:00:00+00:00"
        );
        assert!(month("2026-10", now).is_ok());
        assert!(month("2026-11", now).unwrap_err().contains("not started"));
        for bad in ["2026-9", "2026-13", "september", "2026-09-01", ""] {
            assert!(month(bad, now).is_err(), "{bad}");
        }
        assert_eq!(last_month(now), "2026-09");
        assert_eq!(
            last_month(Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap()),
            "2025-12"
        );
    }

    #[test]
    fn the_csv_has_a_line_per_part_and_defuses_formulas() {
        let measured = BTreeMap::from([(
            "a".to_owned(),
            (
                vec![space("s1", Ok(GIB as u64))],
                vec![assistant("=cmd", Ok(AiSpend::default()))],
            ),
        )]);
        let text = csv("2026-09", "EUR", &split(&prices(), measured));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "month,project,part,item,bytes,cost,currency");
        assert_eq!(lines[1], "2026-09,a,node,,,100.00,EUR");
        assert_eq!(lines[2], "2026-09,a,storage,s1,1073741824,2.00,EUR");
        assert_eq!(lines[3], "2026-09,a,ai,'=cmd,,0.00,EUR");
        assert_eq!(lines[5], "2026-09,,platform-storage,,,18.00,EUR");
        assert_eq!(cell("a,\"b\""), "\"a,\"\"b\"\"\"");
    }
}
