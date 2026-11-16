/**
 * The rows of the one Endpoints grid (T-6169, owner 2026-10-29): the project's own Endpoints, the
 * Endpoints other projects share with it (EP-14) and the references whose Endpoint went away
 * (EP-15), so "Use in this project" and "Stop using" sit in the row menu instead of a second table.
 */
import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "@tanstack/react-router";
import type { Manifest } from "../../api/manifest";
import { usePermissions } from "../../api/permissions";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import type { ResourceTarget } from "../../components/DeleteResourceDialog";
import { EditResourceAction } from "../../components/EditResourceDialog";
import { ManifestYamlDialog } from "../../components/grid/ManifestYamlDialog";
import { referenceTo, sharedReferenceForm } from "../../components/endpoints/sharing";
import { RowActions } from "../../components/ui/RowActions";
import type { RowAction } from "../../components/ui/RowActions";

export interface EndpointRow {
  key: string;
  /** The project that publishes the Endpoint; null for a reference whose Endpoint is gone. */
  source: string | null;
  /** Whether the page's own project publishes it. */
  own: boolean;
  /** The Endpoint, absent only for a gone reference. */
  endpoint: Manifest | null;
  /** This project's SharedSpaceReference to it, when it uses it. */
  reference: Manifest | null;
}

/** Own rows first, then shared rows, then references nothing lists any more. */
export function endpointRows(
  project: string,
  own: Manifest[],
  shared: { source: string; endpoint: Manifest }[],
  references: Manifest[],
): EndpointRow[] {
  const sharedRows = shared.map(({ source, endpoint }) => ({
    key: `${source}/${endpoint.metadata.name}`,
    source,
    own: false,
    endpoint,
    reference:
      referenceTo(references, (endpoint.spec as { slug?: string }).slug ?? "", source, endpoint.metadata.name) ?? null,
  }));
  const used = new Set(sharedRows.map((row) => row.reference?.metadata.name).filter(Boolean));
  return [
    ...own.map((endpoint) => ({ key: `${project}/${endpoint.metadata.name}`, source: project, own: true, endpoint, reference: null })),
    ...sharedRows,
    ...references
      .filter((reference) => !used.has(reference.metadata.name))
      .map((reference) => ({ key: `gone/${reference.metadata.name}`, source: null, own: false, endpoint: null, reference })),
  ];
}

/** The row menu of a shared row or a gone reference: open, YAML, use, alias, stop using. */
export function SharedRowActions({
  project,
  row,
  label,
  onUse,
  useBusy,
}: {
  project: string;
  row: EndpointRow;
  label: string;
  onUse: () => void;
  useBusy: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const permissions = usePermissions(project);
  const [open, setOpen] = useState<"yaml" | "alias" | "stop" | null>(null);
  const deniedReference = permissions.can("SharedSpaceReference", "propose")
    ? undefined
    : t("permissions.denied", { verb: "propose", kind: "SharedSpaceReference" });
  const { endpoint, reference, source } = row;
  const slug = (endpoint?.spec as { slug?: string } | undefined)?.slug ?? "";
  const target: ResourceTarget | null = reference
    ? { project, kind: "SharedSpaceReference", plural: "shared", name: reference.metadata.name, label }
    : null;
  const actions: RowAction[] = [
    ...(endpoint && source
      ? [
          {
            key: "open",
            label: t("resourceList.open"),
            onSelect: () =>
              void navigate({ to: "/projects/$project/$plural/$name", params: { project: source, plural: "endpoints", name: endpoint.metadata.name } }),
          },
          { key: "yaml", label: t("grid.yaml.action"), onSelect: () => setOpen("yaml") },
        ]
      : []),
    ...(endpoint && !reference
      ? [
          {
            key: "use",
            label: t("endpoints.shared.use"),
            onSelect: onUse,
            disabledReason: deniedReference ?? (!slug ? t("endpoints.shared.noAddress") : useBusy ? t("app.busyReason") : undefined),
          },
        ]
      : []),
    ...(reference
      ? [
          { key: "alias", label: t("endpoints.shared.editAlias"), onSelect: () => setOpen("alias"), disabledReason: deniedReference },
          {
            key: "stop",
            label: t("endpoints.shared.stop"),
            tone: "danger" as const,
            onSelect: () => setOpen("stop"),
            disabledReason: permissions.can("SharedSpaceReference", "delete")
              ? undefined
              : t("permissions.denied", { verb: "delete", kind: "SharedSpaceReference" }),
          },
        ]
      : []),
  ];
  return (
    <>
      <RowActions label={label} actions={actions} />
      {open === "yaml" && endpoint ? <ManifestYamlDialog manifest={endpoint} label={label} onClose={() => setOpen(null)} /> : null}
      {open === "alias" && target ? (
        <EditResourceAction target={target} form={sharedReferenceForm(t)} trigger={false} open onOpenChange={(next) => setOpen(next ? "alias" : null)} />
      ) : null}
      {open === "stop" && target ? (
        <DeleteResourceAction target={target} trigger={false} open onOpenChange={(next) => setOpen(next ? "stop" : null)} />
      ) : null}
    </>
  );
}
