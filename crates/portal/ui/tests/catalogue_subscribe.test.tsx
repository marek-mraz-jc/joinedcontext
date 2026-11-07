// covers: src/pages/catalogue/SubscribeMenu.tsx, subscriptions.ts, the dataset page's "Odoberať" menu and its feed
// alternate links (T-5203, EP-125, EP-150, EP-151).
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SubscribeMenu, feedTitle } from "../src/pages/catalogue/SubscribeMenu";
import { calendarUrl } from "../src/pages/catalogue/subscriptions";

const URL = "https://api.dev.joinedcontext.com/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/";
const FEED = {
  name: "kultura",
  title: { sk: "Kultúra", en: "Culture" },
  links: [
    { format: "Atom", mediaType: "application/atom+xml", url: `${URL}feed/v1/kultura/atom.xml` },
    { format: "RSS 2.0", mediaType: "application/rss+xml", url: `${URL}feed/v1/kultura/rss.xml` },
    { format: "JSON Feed", mediaType: "application/feed+json", url: `${URL}feed/v1/kultura/feed.json` },
  ],
};

type Endpoint = Parameters<typeof SubscribeMenu>[0]["endpoint"];
/** A dataset answer; one with no `calendars` is an older Portal's. */
type Answer = Omit<Endpoint, "calendars"> & { calendars?: Endpoint["calendars"] };

function show(endpoint: Answer) {
  return render(
    <I18nextProvider i18n={i18n}>
      <SubscribeMenu endpoint={endpoint as Endpoint} />
    </I18nextProvider>,
  );
}

describe("SubscribeMenu", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("sk");
  });
  afterEach(() => {
    document.head.querySelectorAll("link[data-feed], link[data-calendar]").forEach((link) => link.remove());
  });

  it("names a title in the reader's language and the calendar only where it is served", () => {
    expect(feedTitle(FEED, "en")).toBe("Culture");
    expect(feedTitle({ ...FEED, title: {} }, "sk")).toBe("kultura");
    expect(calendarUrl({ url: URL, representations: ["ngsi-ld"], feeds: [], calendars: [] })).toBeNull();
    expect(calendarUrl({ url: URL, representations: ["ical"], feeds: [], calendars: [] })).toBe(`${URL}ical/v1/events.ics`);
  });

  it("shows nothing for a dataset whose items do not appear over time", () => {
    const { container } = show({ url: URL, representations: ["ngsi-ld", "ogc-features"], feeds: [] });
    expect(container).toBeEmptyDOMElement();
    expect(document.head.querySelectorAll('link[rel="alternate"]')).toHaveLength(0);
  });

  it("puts each feed in the head and opens Kalendár and Novinky", async () => {
    const { unmount } = show({ url: URL, representations: ["ngsi-ld", "ical", "feed"], feeds: [FEED] });
    const alternates = [...document.head.querySelectorAll<HTMLLinkElement>('link[rel="alternate"][data-feed]')];
    expect(alternates.map((link) => [link.type, link.title])).toEqual([
      ["application/atom+xml", "Kultúra (Atom)"],
      ["application/rss+xml", "Kultúra (RSS 2.0)"],
      ["application/feed+json", "Kultúra (JSON Feed)"],
    ]);
    const trigger = screen.getByRole("button", { name: "Odoberať" });
    await userEvent.click(trigger);
    expect(await screen.findByText("Kalendár")).toBeInTheDocument();
    expect(screen.getByText("Novinky")).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Pridať do kalendára" })).toHaveAttribute(
      "href",
      `webcal://api.dev.joinedcontext.com/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/ical/v1/events.ics`,
    );
    expect(screen.getByRole("menuitem", { name: "Kultúra (RSS 2.0)" })).toHaveAttribute("href", FEED.links[1].url);
    unmount();
    expect(document.head.querySelectorAll("link[data-feed]")).toHaveLength(0);
  });

  it("lists each calendar output once by its title, in the menu and in the head (T-5218)", async () => {
    const calendars: Endpoint["calendars"] = [
      { name: "kalendar", title: { sk: "Kalendár podujatí", en: "Events calendar" }, url: `${URL}ical/v1/kalendar.ics` },
      { name: "zasadnutia", title: {}, url: `${URL}ical/v1/zasadnutia.ics` },
    ];
    const { unmount } = show({ url: URL, representations: ["ngsi-ld", "ical"], feeds: [], calendars });
    const heads = [...document.head.querySelectorAll<HTMLLinkElement>('link[rel="alternate"][data-calendar]')];
    expect(heads.map((link) => [link.type, link.title, link.href])).toEqual([
      ["text/calendar", "Kalendár podujatí", calendars[0]!.url],
      ["text/calendar", "zasadnutia", calendars[1]!.url],
    ]);
    await userEvent.click(screen.getByRole("button", { name: "Odoberať" }));
    expect(await screen.findByRole("menuitem", { name: "Pridať do kalendára: Kalendár podujatí" })).toHaveAttribute(
      "href",
      `webcal://api.dev.joinedcontext.com/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/ical/v1/kalendar.ics`,
    );
    expect(screen.getByRole("menuitem", { name: "Stiahnuť zasadnutia (.ics)" })).toHaveAttribute("href", calendars[1]!.url);
    expect(screen.getAllByRole("menuitem")).toHaveLength(4);
    unmount();
    expect(document.head.querySelectorAll("link[data-calendar]")).toHaveLength(0);
  });

  it("an Endpoint that lists iCal with no output offers no calendar", () => {
    const { container } = show({ url: URL, representations: ["ngsi-ld", "ical"], feeds: [], calendars: [] });
    expect(container).toBeEmptyDOMElement();
  });
});
