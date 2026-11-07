import type { Row } from "@joinedcontext/sdk";

/** A local time `days` after `today`'s midnight, as an ISO string the endpoint would send. */
function at(today: Date, days: number, hour: number, minute = 0): string {
  return new Date(today.getFullYear(), today.getMonth(), today.getDate() + days, hour, minute).toISOString();
}

/**
 * A city's events around `today`: two already over, two today, a three-day festival, one far
 * ahead and one whose date is not set yet. Relative to today, so the calendar never opens empty.
 */
export function events(today: Date): Row[] {
  const event = (local: string, name: string, category: string, venue: string, start?: string, end?: string): Row => ({
    id: `urn:ngsi-ld:Event:hel.fi:events:${local}`,
    type: "Event",
    name,
    category,
    venue,
    description: `${name} at ${venue}.`,
    ...(start === undefined ? {} : { startDate: start }),
    ...(end === undefined ? {} : { endDate: end }),
  });
  return [
    event("e1", "Harbour clean-up", "Environment", "South Harbour", at(today, -9, 10), at(today, -9, 13)),
    event("e2", "Library reading night", "Culture", "Oodi Library", at(today, -2, 18), at(today, -2, 20)),
    event("e3", "Farmers' market", "Market", "Market Square", at(today, 0, 9), at(today, 0, 15)),
    event("e4", "City council open session", "Civic", "City Hall", at(today, 0, 17), at(today, 0, 19)),
    event("e5", "Jazz in the park", "Music", "Esplanadi Park", at(today, 1, 19), at(today, 1, 22)),
    event("e6", "Cycling safety workshop", "Mobility", "Kallio Library", at(today, 3, 16), at(today, 3, 18)),
    event("e7", "Design week", "Culture", "Cable Factory", at(today, 5, 10), at(today, 7, 18)),
    event("e8", "Neighbourhood assembly", "Civic", "Vallila Hall", at(today, 12, 18), at(today, 12, 20)),
    event("e9", "Autumn half marathon", "Sport", "Olympic Stadium", at(today, 20, 9), at(today, 20, 14)),
    event("e10", "Winter lights opening", "Culture", "Senate Square", at(today, 34, 17), at(today, 34, 19)),
    event("e11", "Open studios weekend", "Culture", "Suvilahti"),
  ];
}

export const ROWS: Row[] = events(new Date());
