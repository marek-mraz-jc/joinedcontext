import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../api/client";
import { humanizeName } from "../pages/apps/appTitle";
import { Alert, buttonClass } from "../components/ui";

/** The pages that open a form on `?draft=`, and the kinds of draft each one holds (AG-61). */
export const DRAFT_PAGES: Record<string, readonly string[]> = {
  spaces: ["ContextSpace"],
  models: ["DataModel"],
  endpoints: ["Endpoint"],
  pipelines: ["Pipeline"],
  datasources: ["DataSource"],
  dashboards: ["Dashboard", "Layer"],
  policies: ["Policy"],
  subscriptions: ["Subscription"],
  csrs: ["ContextSourceRegistration"],
};

/** The page a kind's drafts open on, or none when no page holds that kind. */
export function draftPageOf(kind: string): string | undefined {
  return Object.keys(DRAFT_PAGES).find((page) => DRAFT_PAGES[page].includes(kind));
}

/**
 * Says so when the draft in the address is not one this page holds (T-2768): a draft of another
 * kind, with a link to its own page, or no draft of that name at all. The page itself opens on
 * a draft it holds and would otherwise land silently on its default screen. The address is read
 * as the page mounts, as the page reads it; `HandOff` remounts both for a new draft.
 */
export function DraftElsewhere({ project, page }: { project: string; page: string }): JSX.Element | null {
  const { t } = useTranslation();
  const [name] = useState(() => new URLSearchParams(window.location.search).get("draft") || null);
  const held = Object.hasOwn(DRAFT_PAGES, page) ? DRAFT_PAGES[page] : undefined;
  const drafts = useQuery({
    queryKey: ["drafts", project],
    enabled: name !== null && held !== undefined,
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/drafts", { params: { path: { project } } })),
  });
  if (name === null || held === undefined || !drafts.data) {
    return null;
  }
  const named = drafts.data.items.filter((draft) => draft.name === name);
  if (named.some((draft) => held.includes(draft.kind))) {
    return null;
  }
  const other = named.map((draft) => draftPageOf(draft.kind)).find((found) => found !== undefined);
  return (
    <Alert
      tone="warning"
      title={t("drafts.elsewhere.title", { name })}
      actions={
        other ? (
          <Link
            to="/projects/$project/$plural"
            params={{ project, plural: other }}
            search={{ draft: name }}
            className={buttonClass("secondary", "sm")}
          >
            {t("drafts.elsewhere.open", { page: t(`nav.${other}`, { defaultValue: humanizeName(other) }) })}
          </Link>
        ) : undefined
      }
    >
      {other ? t("drafts.elsewhere.other") : t("drafts.elsewhere.none")}
    </Alert>
  );
}
