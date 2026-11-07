import { useState } from "react";
import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { ProblemDetails } from "../../api/client";
import type { Change } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import {
  Alert,
  Button,
  Checkbox,
  Field,
  FileDropZone,
  PageHeader,
  safeHref,
  Select,
} from "../../components/ui";
import { usePermissions } from "../../api/permissions";
import type { components } from "../../api/schema";
import { pastedSecrets } from "../../api/pastedSecrets";
import { useBranding } from "../../branding";
import { Combobox } from "../../components/pickers/Combobox";
import { ResourceNamePicker } from "../../components/pickers/ResourceNamePicker";

type ImportReport = components["schemas"]["ImportReport"];

/** What the import does with a resource the project already has (MF-23). */
const POLICIES = ["fail", "skip", "replace", "rename"] as const;
type Policy = (typeof POLICIES)[number];

/** The body an import posts: the file and the options beside it (API/01 §4). */
/** The upload the import route reads: the bundle under `file` and the options as fields (MF-20). */
export function importForm(file: File, namespace: string, orgDomain: string, policy: Policy): FormData {
  const body = new FormData();
  body.append("file", file);
  if (namespace.trim()) {
    body.append("targetNamespace", namespace.trim());
  }
  if (orgDomain.trim()) {
    body.append("orgDomain", orgDomain.trim());
  }
  body.append("conflictPolicy", policy);
  return body;
}

/**
 * Import a bundle into a project (MF-20…MF-24, MF-33).
 *
 * Two steps, and the first is not optional in practice: the check posts the bundle with
 * `?dryRun=All` and the person reads what it would do — created, replaced, skipped, renamed, and
 * whether each file still equals the checksum the bundle carried (MF-42). Only then is there a
 * button that proposes, and what it proposes is one Change for the whole bundle (CC-32).
 *
 * There is no URL field. MF-20 allows one and the Portal refuses it with `501`, because fetching
 * a host a caller names is an egress decision with no policy behind it; a field whose only
 * outcome is that refusal would be a worse answer than saying so once.
 */
export function ImportPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const [file, setFile] = useState<File | null>(null);
  const [namespace, setNamespace] = useState(project);
  const [orgDomain, setOrgDomain] = useState("");
  const branding = useBranding();
  const [policy, setPolicy] = useState<Policy>("fail");
  const [pasted, setPasted] = useState<string[]>([]);
  const [report, setReport] = useState<ImportReport | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const permissions = usePermissions(project);

  const post = async (dryRun: boolean) => {
    if (!file) {
      throw new Error(t("import.noFile"));
    }
    // Through the typed client (UI-07). The document types a multipart body as a string; the
    // client passes the FormData through untouched so the browser writes the boundary.
    const { data, error, response } = await api.POST("/api/v1/projects/{project}/import", {
      params: { path: { project }, query: dryRun ? { dryRun: "All" } : {} },
      body: importForm(file, namespace, orgDomain, policy) as unknown as string,
    });
    if (data === undefined) {
      const answered = (error ?? {}) as Partial<ProblemDetails>;
      // The refusal is the Portal's own sentence — which manifest and why (MF-24) — shown as it
      // came rather than replaced with a summary of it.
      throw new Error(answered.detail ?? answered.title ?? `HTTP ${response.status}`);
    }
    return data;
  };

  const check = useMutation({
    mutationFn: () => post(true),
    onMutate: () => {
      setProblem(null);
      setChange(null);
    },
    onSuccess: (answered) => setReport(answered as unknown as ImportReport),
    onError: (error: Error) => {
      setReport(null);
      setProblem(error.message);
    },
  });

  const propose = useMutation({
    mutationFn: () => post(false),
    onMutate: () => setProblem(null),
    onSuccess: (answered) => setChange(answered as unknown as Change),
    onError: (error: Error) => setProblem(error.message),
  });

  const take = async (chosen: File | null) => {
    setFile(chosen);
    setReport(null);
    setChange(null);
    setProblem(null);
    setPasted([]);
    if (!chosen) {
      return;
    }
    // A zip is bytes; only a text bundle can be read for pasted credentials here, and the
    // server checks both.
    if (/\.(ya?ml|json)$/i.test(chosen.name)) {
      setPasted(pastedSecrets(await chosen.text()));
    }
  };

  const counted = (report: ImportReport) =>
    [
      t("import.report.created", { count: report.created.length }),
      t("import.report.replaced", { count: report.replaced.length }),
      t("import.report.skipped", { count: report.skipped.length }),
      t("import.report.renamed", { count: Object.keys(report.renamed ?? {}).length }),
      t("import.report.files", { count: report.nativeFiles }),
    ].join(" · ");

  // An import is one proposal of every manifest in the bundle: a role that may propose nothing
  // here reads why, not a drop zone whose last step is a 403 (T-4997, T-4263).
  if (permissions.data !== undefined && !permissions.can("*", "propose")) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("import.title")} description={t("import.lead")} />
        <Alert role="status" tone="info">
          {t("form.mayNotCreate")}
        </Alert>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-section">
      <PageHeader title={t("import.title")} description={t("import.lead")} />

      <section aria-labelledby="import-bundle" className="flex flex-col gap-3 rounded border border-border p-4">
        <h2 id="import-bundle" className="text-base font-semibold">
          {t("import.bundle")}
        </h2>
        <FileDropZone
          label={t("import.file")}
          button={t("form.dropOne")}
          hint={t("import.fileHelp")}
          accept=".zip,.yaml,.yml,.json"
          onFiles={(files) => void take(files[0] ?? null)}
        >
          {file ? (
            <p className="text-caption text-fg">{t("form.chosenFiles", { names: file.name })}</p>
          ) : null}
        </FileDropZone>
        <p className="text-caption text-fg-muted">{t("import.noUrl")}</p>

        {pasted.length > 0 ? (
          <Alert role="alert" tone="danger">
            {t("import.pastedSecret", { keys: pasted.join(", ") })}
          </Alert>
        ) : null}

        <div className="flex flex-wrap items-end gap-4">
          <Field id="import-namespace" label={t("import.namespace")} help={t("import.namespaceHelp")}>
            {/* A project the caller reads, or a new name the import creates (ADR-N-033). */}
            <ResourceNamePicker
              id="import-namespace"
              label={t("import.namespace")}
              labelled
              from="projects"
              value={namespace}
              onChange={setNamespace}
              create
            />
          </Field>
          <Field id="import-domain" label={t("import.orgDomain")} help={t("import.orgDomainHelp")}>
            {/* This organization's domain is offered; another one (a city's bundle replayed on a
                staging instance) is typed, and the server checks it is a domain name (PF-10). */}
            <Combobox
              id="import-domain"
              label={t("import.orgDomain")}
              labelled
              value={orgDomain ? [orgDomain] : []}
              placeholder={t("import.orgDomainOwn")}
              options={
                branding.orgDomain
                  ? [{ value: branding.orgDomain, label: branding.orgDomain, group: t("import.orgDomainThis") }]
                  : []
              }
              empty={t("import.orgDomainNone")}
              onChange={(values) => setOrgDomain(values[0] ?? "")}
              create={{ label: (name) => t("import.orgDomainOther", { name }), onCreate: setOrgDomain }}
            />
          </Field>
          <Field id="import-policy" label={t("import.policy")} help={t(`import.policyHelp.${policy}`)}>
            <Select
              id="import-policy"
              value={policy}
              onChange={(event) => setPolicy(event.target.value as Policy)}
            >
              {POLICIES.map((name) => (
                <option key={name} value={name}>
                  {t(`import.policyOption.${name}`)}
                </option>
              ))}
            </Select>
          </Field>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <Button
            onClick={() => check.mutate()}
            disabled={!file || pasted.length > 0}
            disabledReason={pasted.length > 0 ? t("import.clearPasted") : t("import.fileMissing")}
            loading={check.isPending}
          >
            {t("import.check")}
          </Button>
          <Button
            variant="primary"
            onClick={() => propose.mutate()}
            disabled={!report || pasted.length > 0}
            disabledReason={pasted.length > 0 ? t("import.clearPasted") : t("import.checkFirst")}
            loading={propose.isPending}
          >
            {t("import.propose")}
          </Button>
          {!report && file ? (
            <span role="status" className="text-caption text-fg-muted">
              {t("import.checkFirst")}
            </span>
          ) : null}
        </div>
      </section>

      {problem ? (
        <Alert role="alert" tone="danger">
          {problem}
        </Alert>
      ) : null}

      {report ? (
        <section aria-labelledby="import-report" className="flex flex-col gap-2 rounded border border-border p-4">
          <h2 id="import-report" className="text-base font-semibold">
            {t("import.report.title")}
          </h2>
          <p className="text-body">{counted(report)}</p>
          <p className="text-caption text-fg-muted">
            {t("import.report.lane", { lane: t(`lane.${report.lane}`) })}
          </p>
          {report.source ? (
            <p className="text-caption text-fg-muted">{t("import.report.source", { source: report.source })}</p>
          ) : null}
          {(["created", "replaced", "skipped"] as const).map((group) =>
            report[group].length > 0 ? (
              <details key={group}>
                <summary className="cursor-pointer text-caption text-fg-muted">
                  {t(`import.report.${group}`, { count: report[group].length })}
                </summary>
                <ul className="mt-1 list-disc pl-5 text-caption">
                  {report[group].map((name) => (
                    <li key={name} className="font-mono">
                      {name}
                    </li>
                  ))}
                </ul>
              </details>
            ) : null,
          )}
          {Object.keys(report.renamed ?? {}).length > 0 ? (
            <details>
              <summary className="cursor-pointer text-caption text-fg-muted">
                {t("import.report.renamed", { count: Object.keys(report.renamed).length })}
              </summary>
              <ul className="mt-1 list-disc pl-5 text-caption">
                {Object.entries(report.renamed).map(([from, to]) => (
                  <li key={from} className="font-mono">
                    {from} → {to}
                  </li>
                ))}
              </ul>
            </details>
          ) : null}
          {/* MF-42: a transfer that carried no checksums says so, rather than reading as if
              every file were equal. */}
          {report.verified && report.verified.length > 0 ? (
            <p className="text-caption text-fg-muted">
              {t("import.report.verified", {
                equal: report.verified.filter((file) => file.equal).length,
                total: report.verified.length,
              })}
            </p>
          ) : (
            <p className="text-caption text-fg-muted">{t("import.report.unverifiable")}</p>
          )}
          {/* CC-84: what no copy carries, as a checklist of where each thing is set. */}
          <section aria-labelledby="import-needs" className="flex flex-col gap-1">
            <h3 id="import-needs" className="text-sm font-semibold">
              {t("import.report.needsTitle")}
            </h3>
            {(report.needs ?? []).length === 0 ? (
              <p className="text-caption text-fg-muted">{t("import.report.nothingToProvide")}</p>
            ) : (
              <ul className="flex flex-col gap-1 text-caption">
                {(report.needs ?? []).map((need) => (
                  <li key={`${need.kind} ${need.where}`} className="flex flex-wrap items-baseline gap-2">
                    <Checkbox label={<span className="font-mono">{need.where}</span>} />
                    <span className="font-medium">{t(`import.report.need.${need.kind}`)}</span>
                    <span className="text-fg-muted">{need.why}</span>
                    <a className="underline" href={safeHref(need.link)}>
                      {t("import.report.needSet")}
                    </a>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </section>
      ) : null}

      {change ? <ChangeNotice change={change} project={project} /> : null}
    </div>
  );
}
