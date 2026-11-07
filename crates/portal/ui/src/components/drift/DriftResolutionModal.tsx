import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import {
  Alert,
  Badge,
  Button,
  Dialog,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../ui";

/** One drifted seed entity, as `GET /api/v1/projects/{project}/drift` serves it (API/01 §20). */
export interface DriftedEntity {
  space: string;
  id: string;
  drift: "MODIFIED" | "MISSING";
  diff: { path: string; declared: unknown; live?: unknown }[];
  resolutions: string[];
  source: string;
}

/** What a value looks like in the table: JSON, and `—` where the space holds none. */
function shown(value: unknown): string {
  return value === undefined ? "—" : JSON.stringify(value);
}

/**
 * The two resolutions, and only those (UI-26, CC-21, CC-38).
 *
 * **Revert** writes what the repository declares back into the space. **Adopt** proposes the
 * live entity as the seed file's content, reviewed like any other change — it is the only way
 * an out-of-band value becomes durable, which is what keeps Git the truth (CC-21).
 *
 * A resolution the answer does not offer is not shown as a disabled button with a tooltip: it
 * is absent with its reason on the line, because an entity the space no longer holds has
 * nothing to adopt and removing a manifest is an explicit deletion, not a button (CC-19).
 */
export function DriftResolutionModal({
  project,
  entity,
  open,
  onOpenChange,
}: {
  project: string;
  entity: DriftedEntity | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element | null {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [proposed, setProposed] = useState(false);

  const resolve = useMutation({
    mutationFn: async (resolution: "revert" | "adopt") => {
      if (!entity) {
        throw new Error("no entity");
      }
      const path = { project, space: entity.space, id: entity.id };
      return unwrap(
        resolution === "revert"
          ? await api.POST("/api/v1/projects/{project}/drift/{space}/{id}/revert", {
              params: { path },
            })
          : await api.POST("/api/v1/projects/{project}/drift/{space}/{id}/adopt", {
              params: { path },
            }),
      );
    },
    onSuccess: (_result, resolution) => {
      setProposed(resolution === "adopt");
      // The scan runs on the reconciler's tick, so the list here is refetched rather than
      // edited: what the next scan finds is the answer, not what this page assumed.
      void queryClient.invalidateQueries({ queryKey: ["drift", project] });
      if (resolution === "revert") {
        onOpenChange(false);
      }
    },
  });

  if (!entity) {
    return null;
  }

  const close = (next: boolean) => {
    if (!next) {
      setProposed(false);
      resolve.reset();
    }
    onOpenChange(next);
  };

  const failure =
    resolve.error instanceof ApiError
      ? (resolve.error.problem?.detail ?? resolve.error.message)
      : resolve.error
        ? t("app.error.generic")
        : null;
  const may = (resolution: string) => entity.resolutions.includes(resolution);

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      size="md"
      title={t("drift.modal.title", { id: entity.id })}
      description={t(`drift.modal.lead.${entity.drift === "MISSING" ? "missing" : "modified"}`)}
      closeLabel={t("drift.modal.close")}
      footer={
        proposed ? (
          <Button onClick={() => close(false)}>{t("drift.modal.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            {may("adopt") ? (
              <Button
                variant="secondary"
                disabled={resolve.isPending} disabledReason={t("app.busyReason")}
                onClick={() => resolve.mutate("adopt")}
              >
                {t("drift.modal.adopt")}
              </Button>
            ) : null}
            {may("revert") ? (
              <Button disabled={resolve.isPending} disabledReason={t("app.busyReason")} onClick={() => resolve.mutate("revert")}>
                {t("drift.modal.revert")}
              </Button>
            ) : null}
          </>
        )
      }
    >
      <div className="flex flex-col gap-4">
        <div className="flex items-center gap-2">
          <Badge tone={entity.drift === "MISSING" ? "warning" : "info"}>
            {t(`drift.kind.${entity.drift === "MISSING" ? "missing" : "modified"}`)}
          </Badge>
          <span className="text-caption text-fg-muted">{entity.space}</span>
        </div>

        {entity.diff.length > 0 ? (
          <Table caption={t("drift.modal.tableCaption", { space: entity.space })}>
            <TableHead>
              <TableHeaderCell>{t("drift.modal.attribute")}</TableHeaderCell>
              <TableHeaderCell>{t("drift.modal.declared")}</TableHeaderCell>
              <TableHeaderCell>{t("drift.modal.live")}</TableHeaderCell>
            </TableHead>
            <TableBody>
              {entity.diff.map((one) => (
                <TableRow key={one.path}>
                  <TableCell primary className="font-mono">
                    {one.path}
                  </TableCell>
                  <TableCell className="font-mono">{shown(one.declared)}</TableCell>
                  <TableCell className="font-mono">{shown(one.live)}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        ) : null}

        {/* The resolution that is not offered says why on the line, rather than being a
            disabled button somebody hovers to find out (UI-26). */}
        {may("adopt") ? null : (
          <p className="text-caption text-fg-muted">{t("drift.modal.noAdopt")}</p>
        )}

        <p className="text-caption text-fg-muted">
          {t("drift.modal.source", { file: entity.source })}
        </p>

        {proposed ? (
          <Alert tone="success" role="status">
            {t("drift.modal.proposed")}
          </Alert>
        ) : null}
        {failure ? (
          <Alert tone="danger" role="alert">
            {failure}
          </Alert>
        ) : null}
      </div>
    </Dialog>
  );
}
