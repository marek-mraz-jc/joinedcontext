// T-7706: the feed alternates of a dataset page refuse an address that is not https, for a calendar and for a feed.
import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { useFeedAlternates } from "../src/pages/catalogue/SubscribeMenu";

const URL = "https://api.dev.joinedcontext.com/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/";

afterEach(() => {
  document.head.querySelectorAll("link[data-feed], link[data-calendar]").forEach((link) => link.remove());
});

describe("the feed alternates in the page head", () => {
  it("puts a safe feed and calendar in the head and leaves out the ones with a script address", () => {
    const feeds = [
      {
        name: "kultura",
        title: { en: "Culture" },
        links: [
          { format: "Atom", mediaType: "application/atom+xml", url: `${URL}feeds/kultura.atom` },
          { format: "RSS 2.0", mediaType: "application/rss+xml", url: "javascript:alert(1)" },
        ],
      },
    ];
    const calendars = [
      { name: "events", title: {}, url: `${URL}entities.ics` },
      { name: "hostile", title: {}, url: "javascript:alert(2)" },
    ];
    const { unmount } = renderHook(() => useFeedAlternates(feeds, "en", calendars));
    const feedLinks = [...document.head.querySelectorAll<HTMLLinkElement>("link[data-feed]")];
    expect(feedLinks.map((link) => link.title)).toEqual(["Culture (Atom)"]);
    const calendarLinks = [...document.head.querySelectorAll<HTMLLinkElement>("link[data-calendar]")];
    expect(calendarLinks.map((link) => link.dataset.calendar)).toEqual(["events"]);
    unmount();
    expect(document.head.querySelectorAll("link[data-feed], link[data-calendar]")).toHaveLength(0);
  });
});
