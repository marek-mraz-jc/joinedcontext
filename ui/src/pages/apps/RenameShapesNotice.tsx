import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { isChange } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { Alert, Button, InlineError } from "../../components/ui";
import { PermissionGuard } from "../../components/ui/PermissionGuard";

/** The shape names of the previous release and what they are read as (AP-124). */
export const RENAMED_SHAPES: Record<string, string> = { static: "ui", fullstack: "ui-rust" };

/** The Apps of a list still written with an old shape name, with the name each one gets. */
export function oldShapeApps(apps: Manifest[]): { name: string; from: string; to: string }[] {
  return apps.flatMap((app) => {
    const from = (app.spec as { kind?: string } | undefined)?.kind ?? "";
    const to = RENAMED_SHAPES[from];
    return to ? [{ name: app.metadata.name, from, to }] : [];
  });
}

/**
 * The one click of AP-124 (T-2940): when a project still holds an App written `static` or
 * `fullstack`, the Apps page says so and offers to propose one Change that renames them all, in
 * the clicking person's name. Nothing shows when there is nothing to rename, and nothing shows
 * once the Change is proposed: the page's own notice follows it from there.
 */
export function RenameShapesNotice({
  project,
  apps,
  proposed,
  onProposed,
}: {
  project: string;
  apps: Manifest[];
  /** A Change of this page is already shown; the rename is not offered twice. */
  proposed: boolean;
  onProposed: (change: Change) => void;
}): JSX.Element | null {
  const { t } = useTranslation();
  const old = oldShapeApps(apps);
  const rename = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/apps/rename-shapes", {
          params: { path: { project } },
        }),
      ),
    onSuccess: (result) => {
      if (isChange(result)) {
        onProposed(result);
      }
    },
  });
  if (old.length === 0 || proposed || rename.isSuccess) {
    return null;
  }
  const failure = rename.error
    ? rename.error instanceof ApiError
      ? (rename.error.problem?.detail ?? rename.error.message)
      : t("app.error.generic")
    : null;
  return (
    <Alert
      tone="warning"
      actions={
        <PermissionGuard project={project} kind="App" verb="propose">
          <Button size="sm" loading={rename.isPending} onClick={() => rename.mutate()}>
            {t("apps.renameShapes.action")}
          </Button>
        </PermissionGuard>
      }
    >
      <p>{t("apps.renameShapes.notice", { count: old.length })}</p>
      <ul className="mt-1 list-disc pl-5">
        {old.map((app) => (
          <li key={app.name}>
            {t("apps.renameShapes.item", { name: app.name, from: app.from, to: app.to })}
          </li>
        ))}
      </ul>
      {failure ? (
        <InlineError className="mt-2">
          {failure}
        </InlineError>
      ) : null}
    </Alert>
  );
}
