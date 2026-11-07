import { NAV_SECTIONS } from "../components/layout/navigation";
import { HAND_OFF } from "./HandOff";

type Translate = (key: string, values?: Record<string, string>) => string;

/** Project pages outside the sidebar's sections, by their path segment. */
const PROJECT_PAGES: Record<string, string> = {
  models: "nav.models",
  explore: "nav.explore",
  ckan: "nav.ckan",
  // Every page the assistant can open names itself as a person reads it (UI-59, T-1011,
  // T-1018); one it cannot name would read "another page" in the dock's own breadcrumb.
  activity: "nav.activity",
  approvals: "nav.approvals",
  assistant: "nav.assistant",
  apps: "nav.apps",
  // An older answer's `/access` route lands on Project settings (T-2606) and is named as it.
  access: "nav.settings",
};

/**
 * The page a Portal route opens as a person reads it, its title and the resource the route names,
 * never the address itself (UI-45). A route this does not know is "another page".
 */
export function pageOf(route: string, t: Translate): string {
  const [path, query = ""] = route.split("?");
  const segments = path.split("/").filter((segment) => segment !== "");
  const search = new URLSearchParams(query);
  let named = HAND_OFF.map((key) => search.get(key)).find((value) => value !== null && value !== "");
  let title: string | undefined;
  if (path === "/endpoints" || path === "/organization/endpoints") {
    title = t("allEndpoints.title");
  } else if (segments[0] === "projects" && segments.length >= 3) {
    const [, , section, inside] = segments;
    if (section === "spaces" && inside === "complete") {
      title = t("spaces.complete.title");
    } else if (section in PROJECT_PAGES) {
      title = t(PROJECT_PAGES[section]);
    } else {
      const known = NAV_SECTIONS.find((candidate) => candidate.plural === section);
      title = known === undefined ? undefined : t(known.labelKey);
      if (known !== undefined && inside !== undefined) {
        named ??= decoded(inside);
      }
    }
  }
  if (title === undefined) {
    return t("assistant.anotherPage");
  }
  return named === undefined || named === null ? title : t("assistant.pageNamed", { page: title, name: named });
}

/** A route segment decoded, or as it came when its escapes are malformed (`%E0%A4%A`): a route
 * a run navigated to never takes the dock down. */
function decoded(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}
