import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ApiError } from "../api/client";
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

/**
 * Whether a read failed because the project is out of reach (PF-59): its 404 in the API's words,
 * a list hidden in it, or the project itself "not found". A page shows `ProjectHidden` for it,
 * never the raw sentence with a retry that answers the same (UI judge 2026-11-09).
 */
export function projectOutOfReach(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 404 &&
    (error.hidden || /^project '[a-z0-9-]+' not found$/.test(error.problem?.detail ?? ""))
  );
}
