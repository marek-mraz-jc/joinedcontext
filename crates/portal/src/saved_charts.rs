//! A project's saved single charts (`pv-` dashboards, T-6509) moved into one builder dashboard,
//! "Saved charts", proposed as a Change like every builder dashboard so it has a history (T-7161).
//!
//! The rows name no person (migrations/0055), so the dashboard is one per project. A row goes
//! only once the merged file holds a panel drawing the same Endpoint, type, attribute and kind
//! under the same title; every other row stays, with the reason.

use serde::Serialize;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::grafana::{Kind, Visualization};
use crate::grafana_builder::{
    Aggregation, Board, BoardPanel, Grid, Interval, PanelKind, MAX_PANELS, UID_PREFIX,
};

/// The dashboard's title.
pub const TITLE: &str = "Saved charts";

/// The dashboard's uid: one per project, the same on every run, so a second run finds the first.
pub fn uid_of(project: &str) -> String {
    let digest = Sha256::digest(format!("saved-charts\0{project}").as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{UID_PREFIX}{}", &hex[..16])
}

/// A row that stays where it is, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    pub id: String,
    pub title: String,
    pub reason: String,
}

/// What a move proposes: the dashboard, the rows it draws and the rows it leaves.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub board: Board,
    pub moved: Vec<String>,
    pub kept: Vec<Kept>,
}

fn panel_kind(kind: Kind) -> Option<PanelKind> {
    match kind {
        Kind::Timeseries => Some(PanelKind::Timeseries),
        Kind::Stat => Some(PanelKind::Stat),
        Kind::Table => Some(PanelKind::Table),
        // ponytail: the builder has no map panel yet; a map chart stays a saved chart until it has.
        Kind::Map => None,
    }
}

/// The row as a panel at slot `slot`: two half-width panels per row of the grid.
fn panel_of(row: &Visualization, slot: u32, kind: PanelKind) -> BoardPanel {
    BoardPanel {
        id: slot + 1,
        kind,
        title: row.title.clone(),
        description: String::new(),
        grid: Grid {
            x: (slot % 2) * 12,
            y: (slot / 2) * 8,
            w: 12,
            h: 8,
        },
        endpoint: row.endpoint.clone(),
        entity_type: row.entity_type.clone(),
        attribute: row.attribute.clone(),
        aggregation: Aggregation::None,
        interval: Interval::Auto,
        filters: Vec::new(),
        unit: String::new(),
        decimals: None,
        thresholds: None,
        min: None,
        max: None,
        bargauge: None,
        piechart: None,
        barchart: None,
        text: None,
    }
}

fn kept(row: &Visualization, reason: &str) -> Kept {
    Kept {
        id: row.id.clone(),
        title: row.title.clone(),
        reason: reason.to_owned(),
    }
}

/// The dashboard of one project's rows, oldest first so the panels keep the order they were saved in.
pub fn plan(rows: &[Visualization]) -> Plan {
    let mut ordered: Vec<&Visualization> = rows.iter().collect();
    ordered.sort_by(|a, b| (&a.created_at, &a.id).cmp(&(&b.created_at, &b.id)));
    let mut panels = Vec::new();
    let mut moved = Vec::new();
    let mut held = Vec::new();
    for row in ordered {
        match panel_kind(row.kind) {
            None => held.push(kept(
                row,
                "a map chart: the dashboard builder has no map panel yet",
            )),
            Some(_) if panels.len() >= MAX_PANELS => held.push(kept(
                row,
                "the dashboard holds 24 panels; add this one to a dashboard with Add visualization",
            )),
            Some(kind) => {
                panels.push(panel_of(row, panels.len() as u32, kind));
                moved.push(row.id.clone());
            }
        }
    }
    Plan {
        board: Board {
            title: TITLE.to_owned(),
            description: "Each chart saved on its own before dashboards held several.".to_owned(),
            from: "now-6h".to_owned(),
            to: "now".to_owned(),
            refresh: String::new(),
            variables: Vec::new(),
            panels,
        },
        moved,
        kept: held,
    }
}

/// Which rows the merged dashboard draws, so they may go, and which stay, with the reason. Each
/// panel answers for one row only: two equal rows need two panels.
pub fn settled(rows: &[Visualization], merged: &Board) -> (Vec<String>, Vec<Kept>) {
    let mut free: Vec<&BoardPanel> = merged.panels.iter().collect();
    let mut gone = Vec::new();
    let mut held = Vec::new();
    for row in rows {
        let found = panel_kind(row.kind).and_then(|kind| {
            free.iter().position(|panel| {
                panel.kind == kind
                    && panel.title == row.title
                    && panel.endpoint == row.endpoint
                    && panel.entity_type == row.entity_type
                    && panel.attribute == row.attribute
            })
        });
        match found {
            Some(at) => {
                free.remove(at);
                gone.push(row.id.clone());
            }
            None => held.push(kept(
                row,
                "not in the merged Saved charts dashboard, so it is not removed",
            )),
        }
    }
    (gone, held)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grafana_builder::{check_shape, is_builder_uid};

    fn row(id: &str, project: &str, title: &str, kind: Kind, at: &str) -> Visualization {
        Visualization {
            id: id.into(),
            project: project.into(),
            title: title.into(),
            endpoint: "air".into(),
            entity_type: "AirQualityObserved".into(),
            attribute: "pm25".into(),
            kind,
            created_at: at.into(),
        }
    }

    #[test]
    fn a_projects_uid_is_a_builder_uid_and_the_same_on_every_run() {
        assert!(is_builder_uid(&uid_of("helsinki")));
        assert_eq!(uid_of("helsinki"), uid_of("helsinki"));
        assert_ne!(uid_of("helsinki"), uid_of("espoo"));
    }

    #[test]
    fn every_chart_but_a_map_becomes_one_panel_in_the_order_it_was_saved() {
        let rows = [
            row(
                "b",
                "helsinki",
                "PM2.5 now",
                Kind::Stat,
                "2026-11-02T00:00:00Z",
            ),
            row(
                "a",
                "helsinki",
                "PM2.5",
                Kind::Timeseries,
                "2026-11-01T00:00:00Z",
            ),
            row(
                "c",
                "helsinki",
                "Stations",
                Kind::Map,
                "2026-11-03T00:00:00Z",
            ),
            row(
                "d",
                "helsinki",
                "Readings",
                Kind::Table,
                "2026-11-04T00:00:00Z",
            ),
        ];
        let plan = plan(&rows);
        assert_eq!(plan.moved, ["a", "b", "d"]);
        assert_eq!(plan.kept.len(), 1);
        assert_eq!(plan.kept[0].id, "c");
        assert!(plan.kept[0].reason.contains("no map panel"));
        let titles: Vec<&str> = plan.board.panels.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["PM2.5", "PM2.5 now", "Readings"]);
        assert_eq!(plan.board.panels[1].grid.x, 12);
        assert_eq!(plan.board.panels[2].grid.y, 8);
        check_shape(&plan.board).expect("the builder takes the planned dashboard");
    }

    #[test]
    fn over_24_charts_the_rest_stay_with_the_reason() {
        let rows: Vec<Visualization> = (0..26)
            .map(|n| {
                row(
                    &format!("{n:02}"),
                    "helsinki",
                    "PM2.5",
                    Kind::Stat,
                    &format!("2026-11-01T00:00:{n:02}Z"),
                )
            })
            .collect();
        let plan = plan(&rows);
        assert_eq!(plan.board.panels.len(), MAX_PANELS);
        assert_eq!(plan.kept.len(), 2);
        check_shape(&plan.board).expect("24 panels fit");
    }

    #[test]
    fn a_row_goes_only_when_the_merged_file_draws_it_and_each_panel_answers_once() {
        let rows = [
            row(
                "a",
                "helsinki",
                "PM2.5",
                Kind::Timeseries,
                "2026-11-01T00:00:00Z",
            ),
            row(
                "b",
                "helsinki",
                "PM2.5",
                Kind::Timeseries,
                "2026-11-02T00:00:00Z",
            ),
            row("c", "helsinki", "PM10", Kind::Stat, "2026-11-03T00:00:00Z"),
        ];
        let mut merged = plan(&rows[..1]).board;
        // The reviewer retitled nothing but dropped the second copy and the PM10 number.
        let (gone, held) = settled(&rows, &merged);
        assert_eq!(gone, ["a"]);
        assert_eq!(
            held.iter().map(|k| k.id.as_str()).collect::<Vec<_>>(),
            ["b", "c"]
        );
        // A panel retitled, or edited to another attribute, no longer answers for the row.
        merged.panels[0].title = "PM2.5 by station".into();
        let (gone, held) = settled(&rows, &merged);
        assert!(gone.is_empty());
        assert_eq!(held.len(), 3);
        merged.panels[0].title = "PM2.5".into();
        merged.panels[0].attribute = "pm10".into();
        let (gone, held) = settled(&rows, &merged);
        assert!(gone.is_empty());
        assert_eq!(held.len(), 3);
    }
}
