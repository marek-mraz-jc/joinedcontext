import type { components } from "../../api/schema";
import { safeHref } from "../../components/ui";

type Endpoint = components["schemas"]["CatalogueEndpoint"];
type Feed = components["schemas"]["CatalogueFeed"];
type Calendar = components["schemas"]["CatalogueCalendar"];

// What a dataset offers to subscribe to, apart from the menu that offers it: the dataset page
// loads the menu (and its dropdown) only for a dataset that has something (T-5165).

/** The calendar a dataset serves (EP-125): its https address, `null` when it serves none. */
export function calendarUrl(endpoint: Endpoint): string | null {
  return endpoint.representations.includes("ical") ? `${endpoint.url}ical/v1/events.ics` : null;
}

/**
 * The calendars a dataset serves, one per output (API/11 EP-159); an answer of an older Portal
 * has no `calendars` and stands for the one `events.ics`.
 */
export function calendarsOf(endpoint: Endpoint): Calendar[] {
  const listed = (endpoint as Partial<Endpoint>).calendars;
  if (listed) return listed;
  const url = calendarUrl(endpoint);
  return url ? [{ name: "events", title: {}, url }] : [];
}

/** The feeds and the calendars with a safe address a dataset offers; both empty: nothing to subscribe to. */
export function subscriptionsOf(endpoint: Endpoint): { feeds: Feed[]; calendars: Calendar[] } {
  // A dataset answer of an older Portal has no `feeds`.
  const feeds: Feed[] = (endpoint as Partial<Endpoint>).feeds ?? [];
  return { feeds, calendars: calendarsOf(endpoint).filter((calendar) => safeHref(calendar.url)) };
}
