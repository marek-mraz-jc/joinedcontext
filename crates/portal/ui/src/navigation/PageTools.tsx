import { useEffect, useState, useSyncExternalStore } from "react";
import type { JSX } from "react";
import { useRouterState } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { onPageTitle, pageTitle } from "../documentTitle";
import { Button } from "../components/ui/Button";
import { Icon } from "../components/ui/icons";
import { isItem, placeOf, remember, toggleFavourite, usePlaces } from "./places";

/**
 * On the breadcrumb row of every page (UI-90, UI-91): a star that keeps the page among the
 * person's favourites, and the page's own link to send to a colleague. An item page is also
 * remembered among the last ten the person opened. The page's name is the one its header gave
 * the tab (`documentTitle`); before a header named it, there is nothing to star or remember yet.
 * The link is the address as it stands, its tab and filters included, without the language
 * override: the receiver reads it in their own.
 */
export function PageTools(): JSX.Element | null {
  const { t } = useTranslation();
  const { favourites } = usePlaces();
  const [said, setSaid] = useState<string | null>(null);
  const title = useSyncExternalStore(onPageTitle, pageTitle);
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const search = useRouterState({ select: (state) => state.location.searchStr });
  const location = { pathname, search };
  const path = placeOf(location);
  const project = /^\/projects\/([a-z0-9][a-z0-9-]*)/.exec(location.pathname)?.[1];
  const named = title ? (project ? `${title} · ${project}` : title) : undefined;
  const starred = favourites.some((place) => place.path === path);

  useEffect(() => {
    if (named && isItem(path)) void remember({ path, title: named });
  }, [path, named]);

  if (!named) return null;

  // At the click, the address as it stands: a page may keep its filters there beside the router.
  const here = () => placeOf(window.location);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(`${window.location.origin}${here()}`);
      setSaid(t("places.linkCopied"));
    } catch {
      setSaid(t("places.linkNotCopied"));
    }
  };

  return (
    <div className="flex items-center gap-1">
      <Button
        variant="ghost"
        size="sm"
        aria-pressed={starred}
        aria-label={t("places.star")}
        title={t("places.star")}
        onClick={() => void toggleFavourite({ path: here(), title: named })}
      >
        <Icon name="star" className={starred ? "size-4 fill-current text-primary-soft-fg" : "size-4"} />
      </Button>
      <Button variant="ghost" size="sm" aria-label={t("places.copyLink")} title={t("places.copyLink")} onClick={() => void copy()}>
        <Icon name="link" className="size-4" />
      </Button>
      <span aria-live="polite" className="text-caption text-fg-muted">
        {said}
      </span>
    </div>
  );
}
