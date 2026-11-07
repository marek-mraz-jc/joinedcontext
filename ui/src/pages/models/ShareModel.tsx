/**
 * A project's model shared with the organization (DM-77), and the project moving onto the
 * organization's copy once it is there (DM-78, ADR-N-039 §3.2, §3.3).
 *
 * Sharing proposes a red-lane Change of the organization repository that an organization
 * administrator approves; nothing in the project changes. Once the copy is merged, the model's page
 * offers the project's own Change that imports the copy in place of the classes it defines.
 */
import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { isMap, isScalar, isSeq, parseDocument } from "yaml";
import { api, unwrap, reasonOf } from "../../api/client";
import { readModelSource, writeModelSource } from "../../api/datamodelSource";
import { ORG_NAMESPACE } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { usePermissions } from "../../api/permissions";
import { ChangeNotice } from "../../components/ChangeNotice";
import { useOrganizationModels } from "../../components/pickers/organizationModels";
import { Alert, Button, ConfirmDialog, InlineError } from "../../components/ui";
import { organizationImportName } from "./OrganizationModels";

/** The parts of a LinkML schema that define things an import can bring instead. */
const DEFINITIONS = ["classes", "slots", "enums", "types", "subsets"] as const;

/**
 * The model's source importing the organization's copy under `importName` instead of defining
 * what the copy defines (DM-78): `id`, `name`, prefixes and anything the copy lacks stay, so class
 * names and IRIs are the ones the copy carries, byte for byte the ones shared. A source that does
 * not parse is returned as it is.
 */
export function adoptOrganizationCopy(source: string, organizationSource: string, importName: string): string {
  const doc = parseDocument(source);
  const copy = parseDocument(organizationSource);
  if (doc.errors.length > 0 || copy.errors.length > 0 || !isMap(doc.contents)) {
    return source;
  }
  for (const section of DEFINITIONS) {
    const mine = doc.get(section);
    const theirs = copy.get(section);
    if (!isMap(mine) || !isMap(theirs)) continue;
    for (const pair of theirs.items) {
      mine.delete(isScalar(pair.key) ? pair.key.value : pair.key);
    }
    if (mine.items.length === 0) {
      doc.delete(section);
    }
  }
  const imports = doc.get("imports");
  if (!isSeq(imports)) {
    doc.set("imports", doc.createNode(["linkml:types", importName]));
  } else if (!imports.items.some((item) => isScalar(item) && item.value === importName)) {
    imports.add(doc.createNode(importName));
  }
  return doc.toString();
}

/** Whether `source` imports `importName` already. */
function importsAlready(source: string, importName: string): boolean {
  const imports: unknown = parseDocument(source).toJS()?.imports;
  return Array.isArray(imports) && imports.includes(importName);
}

/**
 * "Share with the organization" (DM-77): for a published model and a person who may propose one,
 * with the reason on the disabled button for anyone else.
 */
export function ShareWithOrganization({
  project,
  name,
  version,
  lifecycle,
}: {
  project: string;
  name: string;
  version?: string;
  lifecycle?: string;
}): JSX.Element {
  const { t } = useTranslation();
  const { can } = usePermissions(project);
  const [open, setOpen] = useState(false);
  const share = useMutation({
    mutationFn: async (): Promise<Change> =>
      unwrap(await api.POST("/api/v1/projects/{project}/datamodels/{name}/share", { params: { path: { project, name } } })),
    onSettled: () => setOpen(false),
  });
  const reason =
    lifecycle !== "published"
      ? t("models.share.notPublished")
      : !can("DataModel", "propose")
        ? t("permissions.denied", { verb: "propose", kind: "DataModel" })
        : undefined;
  return (
    <div className="flex flex-col items-end gap-2">
      <Button size="sm" disabled={reason !== undefined} disabledReason={reason} onClick={() => setOpen(true)}>
        {t("models.share.action")}
      </Button>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={t("models.share.title", { name })}
        description={t("models.share.lead", { name, version: version ?? "" })}
        confirmLabel={t("models.share.confirm")}
        tone="primary"
        pending={share.isPending}
        onConfirm={() => share.mutate()}
      />
      {share.data ? <ChangeNotice change={share.data} project={share.data.metadata.namespace ?? ORG_NAMESPACE} /> : null}
      {share.isError ? (
        <Alert tone="danger">
          {t("models.share.failed", { reason: reasonOf(share.error, t("app.error.generic")) })}
        </Alert>
      ) : null}
    </div>
  );
}

/**
 * The offer of DM-78 on a model the organization holds a copy of, shared from here: shown once the
 * copy is merged and while the model does not import it yet.
 */
export function OrganizationCopyOffer({ project, name, source }: { project: string; name: string; source: string }): JSX.Element | null {
  const { t } = useTranslation();
  const { can } = usePermissions(project);
  const models = useOrganizationModels();
  const copy = models.data?.items.find(
    (model) =>
      model.level === "organization" &&
      model.name === name &&
      model.origin?.project === project &&
      model.origin.name === name,
  );
  const importName = copy ? organizationImportName(copy.name, copy.version) : undefined;
  const offered = importName !== undefined && !importsAlready(source, importName);
  const copySource = useQuery({
    queryKey: ["datamodel-source", ORG_NAMESPACE, name],
    enabled: offered,
    retry: false,
    queryFn: () => readModelSource(ORG_NAMESPACE, name),
  });
  const adopt = useMutation({
    mutationFn: async () => {
      const answer = await writeModelSource({
        project,
        name,
        source: adoptOrganizationCopy(source, copySource.data ?? "", importName ?? ""),
        dryRun: false,
      });
      if (answer.kind === "refused") {
        throw new Error(answer.problem.detail ?? answer.problem.title ?? `HTTP ${answer.status}`);
      }
      return answer.kind === "proposed" ? answer.change : undefined;
    },
  });
  if (!offered || copy === undefined) {
    return null;
  }
  const reason = !can("DataModel", "propose")
    ? t("permissions.denied", { verb: "propose", kind: "DataModel" })
    : copySource.data === undefined
      ? t("models.share.copyLoading")
      : undefined;
  return (
    <Alert tone="info">
      <p>{t("models.share.adoptLead", { version: copy.version, importName })}</p>
      {copySource.isError ? (
        <InlineError className="mt-1">
          {t("models.share.copyFailed", { reason: reasonOf(copySource.error, t("app.error.generic")) })}
        </InlineError>
      ) : null}
      <Button
        size="sm"
        className="mt-2"
        disabled={reason !== undefined || adopt.isPending}
        disabledReason={reason}
        onClick={() => adopt.mutate()}
      >
        {t("models.share.adopt")}
      </Button>
      {adopt.data ? <ChangeNotice change={adopt.data} project={project} /> : null}
      {adopt.isError ? (
        <InlineError className="mt-2">
          {t("models.share.failed", { reason: reasonOf(adopt.error, t("app.error.generic")) })}
        </InlineError>
      ) : null}
    </Alert>
  );
}
