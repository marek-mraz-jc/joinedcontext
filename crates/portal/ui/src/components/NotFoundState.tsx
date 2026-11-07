import type { JSX } from "react";
import { Link, useRouterState } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { buttonClass, EmptyState } from "./ui";

/**
 * "There is no page at this address", with the address and a way home (UI-15, UI-16): the router's
 * own fallback and a project section the API does not know say the same thing (T-2749).
 */
export function NotFoundState(): JSX.Element {
  const { t } = useTranslation();
  const path = useRouterState({ select: (state) => state.location.pathname });
  return (
    <EmptyState
      icon="search"
      // The page is nothing but this, so its title is the page's heading (WCAG 1.3.1, T-3150).
      heading={1}
      title={t("app.notFound.title")}
      description={t("app.notFound.description", { path })}
      action={
        <Link to="/" className={buttonClass("primary", "md")}>
          {t("app.notFound.home")}
        </Link>
      }
    />
  );
}
