import { useEffect, useMemo } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "../../api/schema";
import { webcalUrl } from "../../components/endpoints/links";
import { Button, Icon, Menu, MenuContent, MenuItem, MenuLabel, MenuSeparator, MenuTrigger, safeHref } from "../../components/ui";
import { subscriptionsOf } from "./subscriptions";

type Endpoint = components["schemas"]["CatalogueEndpoint"];
type Feed = components["schemas"]["CatalogueFeed"];
type Calendar = components["schemas"]["CatalogueCalendar"];

/** An output's title in the reader's language, else its first, else its name. */
export function feedTitle(feed: Pick<Feed, "name" | "title">, language: string): string {
  return feed.title[language] ?? Object.values(feed.title)[0] ?? feed.name;
}

/** One calendar's two rows: subscribe in a calendar app (webcal) and download the file. */
function CalendarItems({ calendar, label }: { calendar: Calendar; label: string | null }): JSX.Element | null {
  const { t } = useTranslation();
  const https = safeHref(calendar.url);
  if (!https) return null;
  // `webcal://` of the checked https address; safeHref itself refuses the webcal scheme.
  const safe = webcalUrl(https);
  return (
    <>
      <MenuItem asChild>
        <a href={safe}>
          <Icon name="link" className="size-4" />
          {label ? t("catalogue.subscribe.calendarAppNamed", { title: label }) : t("catalogue.subscribe.calendarApp")}
        </a>
      </MenuItem>
      <MenuItem asChild>
        <a href={safeHref(calendar.url)} type="text/calendar">
          <Icon name="external" className="size-4" />
          {label ? t("catalogue.subscribe.calendarFileNamed", { title: label }) : t("catalogue.subscribe.calendarFile")}
        </a>
      </MenuItem>
    </>
  );
}

/**
 * Puts one `<link rel="alternate">` per feed output and format, and one per calendar, into the page
 * head while the page is open, so a feed reader or calendar given the page's address finds them
 * (EP-150, EP-151, API/11 EP-159).
 */
export function useFeedAlternates(feeds: Feed[], language: string, calendars: Calendar[] = []): void {
  useEffect(() => {
    const ics = calendars.flatMap((calendar) => {
      const href = safeHref(calendar.url);
      if (!href) return [];
      const element = document.createElement("link");
      element.rel = "alternate";
      element.type = "text/calendar";
      element.title = feedTitle(calendar, language);
      element.href = href;
      element.dataset.calendar = calendar.name;
      return [element];
    });
    const links = feeds.flatMap((feed) =>
      feed.links.flatMap((link) => {
        const href = safeHref(link.url);
        if (!href) return [];
        const element = document.createElement("link");
        element.rel = "alternate";
        element.type = link.mediaType;
        element.title = `${feedTitle(feed, language)} (${link.format})`;
        element.href = href;
        element.dataset.feed = feed.name;
        return [element];
      }),
    );
    const all = [...links, ...ics];
    all.forEach((element) => document.head.appendChild(element));
    return () => all.forEach((element) => element.remove());
  }, [feeds, language, calendars]);
}

/**
 * "Subscribe": the dataset's calendar and its feed outputs, grouped as Calendar and News, each a
 * link a reader opens or copies into a calendar or feed reader (EP-125, EP-150). A dataset with
 * neither shows nothing: feeds are offered only where items appear over time (owner 2026-10-22).
 */
export function SubscribeMenu({ endpoint }: { endpoint: Endpoint }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const language = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const { feeds, calendars } = useMemo(() => subscriptionsOf(endpoint), [endpoint]);
  useFeedAlternates(feeds, language, calendars);
  if (calendars.length === 0 && feeds.length === 0) return null;
  // One calendar keeps the plain labels; several are told apart by their titles.
  const titled = calendars.length > 1 || calendars.some((calendar) => Object.keys(calendar.title).length > 0);
  return (
    <Menu>
      <MenuTrigger asChild>
        <Button variant="secondary">
          <Icon name="share" className="size-4" />
          {t("catalogue.subscribe.button")}
        </Button>
      </MenuTrigger>
      <MenuContent align="end" collisionPadding={16} className="min-w-64 max-w-sm">
        {calendars.length > 0 ? <MenuLabel>{t("catalogue.subscribe.calendar")}</MenuLabel> : null}
        {calendars.map((calendar) => (
          <CalendarItems key={calendar.name} calendar={calendar} label={titled ? feedTitle(calendar, language) : null} />
        ))}
        {calendars.length > 0 && feeds.length > 0 ? <MenuSeparator /> : null}
        {feeds.length > 0 ? <MenuLabel>{t("catalogue.subscribe.news")}</MenuLabel> : null}
        {feeds.map((feed) =>
          feed.links.map((link) => (
            <MenuItem key={`${feed.name}-${link.format}`} asChild>
              <a href={safeHref(link.url)} type={link.mediaType}>
                <Icon name="link" className="size-4" />
                {t("catalogue.subscribe.feed", { title: feedTitle(feed, language), format: link.format })}
              </a>
            </MenuItem>
          )),
        )}
        <p className="max-w-72 px-2.5 py-1.5 text-caption text-fg-muted">{t("catalogue.subscribe.hint")}</p>
      </MenuContent>
    </Menu>
  );
}
