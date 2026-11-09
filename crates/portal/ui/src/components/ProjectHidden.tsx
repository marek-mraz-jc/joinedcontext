import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { Alert } from "./ui";

/**
 * What a person reads where a project's `permissions/me` answered 404 (PF-59, jc-portal-refusals):
 * a project nobody shared with them and one that does not exist refuse in the same words, so the
 * page tells nothing of either, with the way to the start instead of a control that would refuse.
 */
export function ProjectHidden(): JSX.Element {
  const { t } = useTranslation();
  return (
    <Alert role="status" tone="info">
      <span className="flex flex-col gap-2">
        <span>{t("form.projectHidden")}</span>
        <Link to="/" className="w-fit font-medium underline hover:no-underline">
          {t("app.notFound.home")}
        </Link>
      </span>
    </Alert>
  );
}
