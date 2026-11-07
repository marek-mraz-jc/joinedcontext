import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { usePermissions } from "../../api/permissions";
import { DriftResolutionModal } from "../../components/drift/DriftResolutionModal";
import type { DriftedEntity } from "../../components/drift/DriftResolutionModal";
import {
  Badge,
  Button,
  PageFailed,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";

interface DriftList {
  observedAt?: string;
  items: DriftedEntity[];
}

function isEntity(value: unknown): value is DriftedEntity {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const entity = value as Record<string, unknown>;
  return (
    typeof entity.space === "string" &&
    typeof entity.id === "string" &&
    (entity.drift === "MODIFIED" || entity.drift === "MISSING") &&
    Array.isArray(entity.diff) &&
    entity.diff.every((one) => typeof one === "object" && one !== null && typeof (one as { path?: unknown }).path === "string") &&
    Array.isArray(entity.resolutions) &&
    entity.resolutions.every((one) => typeof one === "string") &&
    typeof entity.source === "string"
  );
}

/** The list as `GET …/drift` answers it; an entry of another shape is not shown rather than guessed at. */
export function asDriftList(body: unknown): DriftList {
  const value = (typeof body === "object" && body !== null ? body : {}) as {
    metadata?: { observedAt?: unknown };
    items?: unknown;
  };
  const observedAt = value.metadata?.observedAt;
  return {
    observedAt: typeof observedAt === "string" ? observedAt : undefined,
    items: Array.isArray(value.items) ? value.items.filter(isEntity) : [],
  };
}

/**
 * What of this space drifted from the repository at the last scan (UI-25, UI-26, CC-21; T-2867),
 * and the way to pick a side: Resolve opens the two resolutions for a person who may propose an
 * Entity. Before the first scan it says so, never "nothing drifted".
 */
export function SpaceDrift({ project, space }: { project: string; space: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const permissions = usePermissions(project);
  const [open, setOpen] = useState<DriftedEntity | null>(null);
  // The key the modal refetches after a resolution, so the list shows what the next scan finds.
  const drift = useQuery({
    queryKey: ["drift", project],
    queryFn: async () =>
      asDriftList(
        await unwrap(
          await api.GET("/api/v1/projects/{project}/drift", {
            params: { path: { project } },
          }),
        ),
      ),
  });

  if (drift.isPending) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  if (drift.isError) {
    return (
      <PageFailed
        error={drift.error}
        onRetry={() => {
          void drift.refetch();
        }}
      />
    );
  }
  if (drift.data.observedAt === undefined) {
    return <p className="text-body text-fg-muted">{t("drift.section.notYet")}</p>;
  }

  const when = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(
    new Date(drift.data.observedAt),
  );
  const items = drift.data.items.filter((entity) => entity.space === space);
  const mayResolve = permissions.can("Entity", "propose");

  return (
    <div className="space-y-3">
      <p className="text-body text-fg">
        {items.length === 0 ? t("drift.section.clean") : t("drift.section.found", { count: items.length })}
      </p>
      <p className="text-caption text-fg-muted">{t("drift.section.observed", { when })}</p>
      {items.length > 0 ? (
        <Table caption={t("drift.section.caption", { space })}>
          <TableHead>
            <TableHeaderCell>{t("drift.section.entity")}</TableHeaderCell>
            <TableHeaderCell>{t("drift.section.kind")}</TableHeaderCell>
            <TableHeaderCell>{t("drift.section.attributes")}</TableHeaderCell>
            <TableHeaderCell>{t("drift.section.resolution")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {items.map((entity) => (
              <TableRow key={entity.id}>
                <TableCell primary className="font-mono text-caption [overflow-wrap:anywhere]">
                  {entity.id}
                </TableCell>
                <TableCell>
                  <Badge tone={entity.drift === "MISSING" ? "warning" : "info"}>
                    {t(`drift.kind.${entity.drift === "MISSING" ? "missing" : "modified"}`)}
                  </Badge>
                </TableCell>
                <TableCell className="font-mono text-caption">
                  {entity.diff.length > 0
                    ? entity.diff.map((one) => one.path).join(", ")
                    : t("drift.section.wholeEntity")}
                </TableCell>
                <TableCell>
                  {mayResolve ? (
                    <Button
                      size="sm"
                      variant="secondary"
                      aria-label={t("drift.section.resolveLabel", { id: entity.id })}
                      onClick={() => setOpen(entity)}
                    >
                      {t("drift.section.resolve")}
                    </Button>
                  ) : (
                    <span className="text-caption text-fg-muted">{t("drift.section.mayNotResolve")}</span>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : null}
      <DriftResolutionModal
        project={project}
        entity={open}
        open={open !== null}
        onOpenChange={(next) => {
          if (!next) {
            setOpen(null);
          }
        }}
      />
    </div>
  );
}
