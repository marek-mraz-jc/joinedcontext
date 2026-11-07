// SPDX-License-Identifier: EUPL-1.2
//! The query guide table of the Smart Data Model types the platform serves (T-4486, T-4537): per
//! type, its time axis, its place and the attributes worth filtering on. The MCP surface writes it
//! as text, the OGC surface as the `x-ogc-role` of each queryable; each names only the attributes
//! its view shows, so the table itself names no view.

/// A time axis: one instant attribute, or a range of a start and an end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Time {
    /// One attribute holds the instant.
    Instant(&'static str),
    /// A start attribute and an end attribute.
    Range(&'static str, &'static str),
}

/// How to ask one type.
#[derive(Debug)]
pub struct Guide {
    /// The Entity type, as the Smart Data Model names it.
    pub entity_type: &'static str,
    /// Time axis candidates in order; the first the view shows is the axis.
    pub time: &'static [Time],
    /// Place candidates in order; every shown one is named.
    pub place: &'static [&'static str],
    /// The attributes worth filtering on, most useful first.
    pub filters: &'static [&'static str],
}

/// The SDM types the platform serves (ponytail: grown when a type without a guide is filed).
pub const GUIDES: &[Guide] = &[
    Guide {
        entity_type: "Event",
        time: &[
            Time::Range("startDate", "endDate"),
            Time::Instant("startDate"),
        ],
        place: &["location", "address"],
        filters: &["category", "eventType", "name", "organizer"],
    },
    Guide {
        entity_type: "AirQualityObserved",
        time: &[Time::Instant("dateObserved")],
        place: &["location", "address"],
        filters: &["pm10", "pm25", "no2", "o3", "airQualityIndex"],
    },
    Guide {
        entity_type: "WeatherObserved",
        time: &[Time::Instant("dateObserved")],
        place: &["location", "address"],
        filters: &[
            "temperature",
            "precipitation",
            "windSpeed",
            "relativeHumidity",
        ],
    },
    Guide {
        entity_type: "WeatherForecast",
        time: &[
            Time::Range("validFrom", "validTo"),
            Time::Instant("dateIssued"),
        ],
        place: &["location", "address"],
        filters: &["temperature", "precipitationProbability", "windSpeed"],
    },
    Guide {
        entity_type: "Alert",
        time: &[
            Time::Range("validFrom", "validTo"),
            Time::Instant("dateIssued"),
        ],
        place: &["location", "address"],
        filters: &["category", "subCategory", "severity"],
    },
    Guide {
        entity_type: "KeyPerformanceIndicator",
        time: &[
            Time::Instant("refPeriod"),
            Time::Instant("dateObserved"),
            Time::Range("calculationPeriod[from]", "calculationPeriod[to]"),
        ],
        place: &["refArea", "area", "location", "address"],
        filters: &[
            "indicator",
            "refArea",
            "dimensionKey",
            "kpiValue",
            "category",
        ],
    },
    Guide {
        entity_type: "Vehicle",
        time: &[Time::Instant("location.observedAt")],
        place: &["location"],
        filters: &["vehicleType", "speed", "serviceStatus"],
    },
    Guide {
        entity_type: "ParkingSpot",
        time: &[Time::Instant("status.observedAt")],
        place: &["location", "address"],
        filters: &["status", "category"],
    },
    Guide {
        entity_type: "OffStreetParking",
        time: &[Time::Instant("availableSpotNumber.observedAt")],
        place: &["location", "address"],
        filters: &["availableSpotNumber", "totalSpotNumber", "category"],
    },
    Guide {
        entity_type: "BikeHireDockingStation",
        time: &[Time::Instant("availableBikeNumber.observedAt")],
        place: &["location", "address"],
        filters: &["availableBikeNumber", "freeSlotNumber", "status"],
    },
    Guide {
        entity_type: "TrafficFlowObserved",
        time: &[
            Time::Range("dateObservedFrom", "dateObservedTo"),
            Time::Instant("dateObserved"),
        ],
        place: &["location", "address"],
        filters: &["intensity", "averageVehicleSpeed", "congested"],
    },
    Guide {
        entity_type: "Article",
        time: &[Time::Instant("dateGmt")],
        place: &[],
        filters: &["name", "isPartOf"],
    },
    Guide {
        entity_type: "PointOfInterest",
        time: &[],
        place: &["location", "address"],
        filters: &["category", "name"],
    },
];

/// The long text attributes of a type (T-4582): a read leaves them out unless the caller names
/// them, so a page of 50 Articles is headlines, not 240 KB of bodies.
const LONG_TEXT: &[(&str, &[&str])] = &[("Article", &["content"])];

/// The long text attributes of `entity_type`; empty for a type without one.
#[must_use]
pub fn long_text(entity_type: &str) -> &'static [&'static str] {
    LONG_TEXT
        .iter()
        .find(|(t, _)| *t == entity_type)
        .map_or(&[], |(_, attrs)| *attrs)
}

impl Guide {
    /// The first time axis whose attributes all pass `shows`.
    pub fn time_in(&self, shows: impl Fn(&str) -> bool) -> Option<Time> {
        self.time.iter().copied().find(|time| match time {
            Time::Instant(a) => shows(a),
            Time::Range(a, b) => shows(a) && shows(b),
        })
    }
}

/// The guide of `entity_type`, if the table knows it.
#[must_use]
pub fn of(entity_type: &str) -> Option<&'static Guide> {
    GUIDES.iter().find(|g| g.entity_type == entity_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_shown_axis_wins_and_a_range_needs_both_ends() {
        let event = of("Event");
        assert_eq!(
            event.and_then(|g| g.time_in(|a| a != "nothing")),
            Some(Time::Range("startDate", "endDate"))
        );
        assert_eq!(
            event.and_then(|g| g.time_in(|a| a == "startDate")),
            Some(Time::Instant("startDate"))
        );
        assert_eq!(event.and_then(|g| g.time_in(|_| false)), None);
        assert!(of("Thing").is_none());
    }

    #[test]
    fn an_article_body_is_long_text_and_other_types_have_none() {
        assert_eq!(long_text("Article"), ["content"]);
        assert_eq!(long_text("Event"), [] as [&str; 0]);
        assert_eq!(long_text("Thing"), [] as [&str; 0]);
    }
}
