// SPDX-License-Identifier: EUPL-1.2
//! The model the property test and the bench parse against: buildings with an address, rooms,
//! tags, sensors and an owner.

use odata_url::{Member, Model};

/// Buildings and their sensors.
pub struct Service;

impl Model for Service {
    fn entity_set(&self, name: &str) -> Option<&str> {
        match name {
            "Buildings" => Some("Building"),
            "Sensors" => Some("Sensor"),
            _ => None,
        }
    }

    fn member(&self, ty: &str, name: &str) -> Option<Member<'_>> {
        let primitive = Member::Primitive { collection: false };
        Some(match (ty, name) {
            ("Building", "id" | "height" | "name" | "built" | "location")
            | ("Address", "street" | "city")
            | ("Room", "area")
            | ("Sensor", "value")
            | ("Person", "name") => primitive,
            ("Building", "tags") => Member::Primitive { collection: true },
            ("Building", "address") => Member::Complex {
                ty: "Address",
                collection: false,
            },
            ("Building", "rooms") => Member::Complex {
                ty: "Room",
                collection: true,
            },
            ("Building", "sensors") => Member::Navigation {
                ty: "Sensor",
                collection: true,
            },
            ("Building" | "Sensor", "owner") => Member::Navigation {
                ty: "Person",
                collection: false,
            },
            _ => return None,
        })
    }
}
