import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueries, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { useProjects } from "../../api/projects";
import { ChangeNotice } from "../../components/ChangeNotice";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import {
  Alert,
  Button,
  buttonClass,
  Dialog,
  EmptyState,
  Field,
  FilePicker,
  Icon,
  Input,
  Select,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "../../components/ui";

/** An App name, as the server takes it: a DNS-1123 label of at most 63 characters. */
const APP_NAME = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/;

/** What the dry run of an App import answers: the one repository it would create. */
interface Plan {
  repositories: { name: string; role: string; repository: string; head: string }[];
}

/** The repository an App builds from, the last segment of `source.git.url`; none without one. */
function repositoryOf(app: Manifest): string | null {
  const source = (app.spec as { source?: { git?: { url?: unknown } } } | null)?.source;
  const url = source?.git?.url;
  if (typeof url !== "string" || url.trim() === "") {
    return null;
  }
  const last = url.replace(/\/+$/, "").split("/").pop() ?? "";
  return last.replace(/\.git$/, "") || null;
}

/** The problem document's words, or the status when the answer is not one. */
function refusal(error: unknown, response: Response): string {
  const problem = (error ?? {}) as { detail?: string; title?: string };
  return problem.detail ?? problem.title ?? (response.statusText || `HTTP ${response.status}`);
}

/** The server's file name from `Content-Disposition`, else the App's own. */
function filename(response: Response, name: string): string {
  const header = response.headers.get("content-disposition") ?? "";
  return /filename="([^"]+)"/.exec(header)?.[1] ?? `${name}-app.zip`;
}

/** One App's archive, fetched through the typed client (UI-07) and saved; a refusal throws. */
async function download(project: string, name: string): Promise<void> {
  const { data, error, response } = await api.GET("/api/v1/projects/{project}/apps/{name}/export", {
    params: { path: { project, name } },
    parseAs: "blob",
  });
  if (data === undefined) {
    throw new Error(refusal(error, response));
  }
  const href = URL.createObjectURL(data);
  const link = document.createElement("a");
  link.href = href;
  link.download = filename(response, name);
  link.click();
  URL.revokeObjectURL(href);
}

async function post(project: string, file: File, name: string, dryRun: boolean): Promise<unknown> {
  const body = new FormData();
  body.set("file", file);
  if (name.trim() !== "") {
    body.set("name", name.trim());
  }
  // The document types a multipart body as a string; the FormData passes through so the
  // browser writes the boundary, and the typed client adds the csrf header.
  const { data, error, response } = await api.POST("/api/v1/projects/{project}/import", {
    params: { path: { project }, query: dryRun ? { format: "app", dryRun: "All" } : { format: "app" } },
    body: body as unknown as string,
  });
  if (data === undefined) {
    throw new Error(refusal(error, response));
  }
  return data;
}

/**
 * Importing one App from its export (UI-87, T-2879): the project it lands in, the archive and the
 * name it takes there are checked first, which answers the repository the import creates; then
 * the same check and the import, which proposes the App as the project's red-lane change.
 */
export function ImportAppDialog({
  open,
  onOpenChange,
  projects,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  projects: string[];
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const queryClient = useQueryClient();
  const [project, setProject] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [name, setName] = useState("");
  const [plan, setPlan] = useState<Plan | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const badName = name.trim() !== "" && (!APP_NAME.test(name.trim()) || name.trim().length > 63);

  const check = useMutation({
    mutationFn: async () => {
      if (!file) {
        throw new Error(t("appImport.noFile"));
      }
      return (await post(project, file, name, true)) as Plan;
    },
    onSuccess: (answered) => setPlan(answered),
  });

  const land = useMutation({
    mutationFn: async () => {
      if (!file || !plan) {
        throw new Error(t("appImport.noFile"));
      }
      // The dry run is the check the import is held to, over this very archive and name (PF-57).
      await post(project, file, name, true);
      return (await post(project, file, name, false)) as Change;
    },
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "apps") });
      setChange(result);
    },
  });

  const close = (next: boolean) => {
    if (!next) {
      setProject("");
      setFile(null);
      setName("");
      setPlan(null);
      setChange(null);
      check.reset();
      land.reset();
    }
    onOpenChange(next);
  };

  const failure = (check.error ?? land.error)?.message ?? null;
  const missing = file === null ? t("appImport.noFile") : project === "" ? t("appImport.noProject") : null;

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      size="lg"
      title={t("appImport.title")}
      description={t("appImport.lead")}
      closeLabel={t("resourceDelete.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            {plan ? (
              <Button variant="primary" loading={land.isPending} onClick={() => land.mutate()}>
                {t("appImport.import")}
              </Button>
            ) : (
              <Button
                variant="primary"
                loading={check.isPending}
                disabled={missing !== null || badName}
                disabledReason={missing ?? (badName ? t("appImport.nameInvalid") : undefined)}
                onClick={() => check.mutate()}
              >
                {t("appImport.check")}
              </Button>
            )}
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={project} />
      ) : (
        <div className="flex flex-col gap-4">
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
          <Field id={`${ids}-project`} label={t("appImport.project")} help={t("appImport.projectHint")} required>
            <Select
              id={`${ids}-project`}
              value={project}
              disabled={plan !== null}
              title={plan !== null ? t("appImport.checked") : undefined}
              onChange={(event) => setProject(event.target.value)}
            >
              <option value="">{t("appImport.chooseProject")}</option>
              {projects.map((choice) => (
                <option key={choice} value={choice}>
                  {choice}
                </option>
              ))}
            </Select>
          </Field>
          <div className="flex flex-col gap-1">
            {plan === null ? (
              <FilePicker label={t("appImport.file")} accept=".zip,application/zip" onFile={setFile}>
                <span className={buttonClass("secondary", "sm")}>{t("appImport.file")}</span>
              </FilePicker>
            ) : null}
            <p className="text-caption text-fg-muted">
              {file ? t("appImport.chosen", { name: file.name }) : t("appImport.fileHint")}
            </p>
          </div>
          <Field
            id={`${ids}-name`}
            label={t("appImport.name")}
            help={t("appImport.nameHint")}
            errors={badName ? [t("appImport.nameInvalid")] : undefined}
          >
            <Input
              id={`${ids}-name`}
              value={name}
              autoComplete="off"
              spellCheck={false}
              disabled={plan !== null}
              title={plan !== null ? t("appImport.checked") : undefined}
              onChange={(event) => setName(event.target.value)}
            />
          </Field>
          {plan ? (
            <section className="flex flex-col gap-2" aria-labelledby={`${ids}-plan`}>
              <h3 id={`${ids}-plan`} className="text-body font-semibold text-fg">
                {t("appImport.repository")}
              </h3>
              <ul className="list-disc pl-5 text-body text-fg">
                {plan.repositories.map((repository) => (
                  <li key={repository.repository}>
                    {t("projectImport.repository", {
                      repository: repository.repository,
                      head: repository.head.slice(0, 7),
                    })}
                  </li>
                ))}
              </ul>
              <p className="text-body text-fg-muted">{t("appImport.redLane")}</p>
            </section>
          ) : null}
        </div>
      )}
    </Dialog>
  );
}

/**
 * Administration → Applications (UI-75, UI-87, T-2879): every App of every project the person
 * reads, with its project, lifecycle and the repository it builds from; one App exports as its
 * repository's git bundle beside its manifest, and an export imports into a project under a name.
 */
export function OrganizationApplications(): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const projects = useProjects();
  const names = projects.data ?? [];
  const lists = useQueries({
    queries: names.map((project) => ({
      queryKey: queryKeys.list(project, "apps"),
      queryFn: async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: "apps" } },
          }),
        ),
    })),
  });
  const [importing, setImporting] = useState(false);
  const [exporting, setExporting] = useState<string | null>(null);
  const [refused, setRefused] = useState<string | null>(null);

  const rows = names.flatMap((project, index) =>
    asManifests(lists[index]?.data?.items ?? []).map((app) => ({ project, app })),
  );
  const pending = projects.isPending || lists.some((list) => list.isPending);
  const failed = lists.find((list) => list.isError)?.error;

  const exportOne = async (project: string, name: string) => {
    setRefused(null);
    setExporting(`${project}/${name}`);
    try {
      await download(project, name);
    } catch (error) {
      setRefused(t("appExport.refused", { name, reason: error instanceof Error ? error.message : String(error) }));
    } finally {
      setExporting(null);
    }
  };

  return (
    <section className="space-y-4" aria-labelledby="organization-applications-heading">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 id="organization-applications-heading" className="text-title font-semibold text-fg">
            {t("organization.applications.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("organization.applications.lead")}</p>
        </div>
        {/* Importing an App is here and nowhere else (UI-87). */}
        <Button
          variant="secondary"
          size="sm"
          icon={<Icon name="import" className="size-4" />}
          disabled={names.length === 0}
          disabledReason={names.length === 0 ? t("appImport.noProjects") : undefined}
          onClick={() => setImporting(true)}
        >
          {t("appImport.button")}
        </Button>
        <ImportAppDialog open={importing} onOpenChange={setImporting} projects={names} />
      </div>
      {refused ? (
        <Alert tone="danger" role="alert">
          {refused}
        </Alert>
      ) : null}
      {projects.isError || failed ? (
        <Alert tone="danger" role="alert">
          {(() => {
            const error = projects.error ?? failed;
            return error instanceof ApiError
              ? (error.problem?.detail ?? error.message)
              : t("app.error.generic");
          })()}
        </Alert>
      ) : (
        <Table
          data-records=""
          caption={t("organization.applications.caption")}
          status={pending ? t("app.loading") : undefined}
        >
          <TableHead>
            <TableHeaderCell>{t("organization.applications.name")}</TableHeaderCell>
            <TableHeaderCell>{t("organization.projects.name")}</TableHeaderCell>
            <TableHeaderCell>{t("organization.applications.lifecycle")}</TableHeaderCell>
            <TableHeaderCell>{t("organization.applications.source")}</TableHeaderCell>
            <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
          </TableHead>
          {pending ? (
            <TableSkeleton columns={5} />
          ) : (
            <TableBody>
              {rows.length === 0 ? (
                <TableEmpty columns={5}>
                  <EmptyState
                    bare
                    title={t("organization.applications.empty")}
                    description={t("organization.applications.emptyHint")}
                  />
                </TableEmpty>
              ) : (
                rows.map(({ project, app }) => {
                  const name = app.metadata.name;
                  const title = localized(app.metadata.title, locale, "");
                  const repository = repositoryOf(app);
                  const lifecycle = (app.spec as { lifecycle?: string } | null)?.lifecycle;
                  return (
                    <TableRow key={`${project}/${name}`}>
                      <TableCell primary>
                        <Link
                          data-row-link=""
                          to="/projects/$project/$plural"
                          params={{ project, plural: "apps" }}
                          className="focus-ring rounded-sm font-medium text-primary-soft-fg underline underline-offset-2 hover:no-underline"
                        >
                          {title || name}
                        </Link>
                        {title ? <span className="ml-2 text-caption text-fg-muted">{name}</span> : null}
                      </TableCell>
                      <TableCell>{project}</TableCell>
                      <TableCell>
                        {lifecycle ? (
                          <LifecycleBadge kind="appLifecycle" value={lifecycle} />
                        ) : (
                          <span className="text-fg-muted">{t("organization.applications.noLifecycle")}</span>
                        )}
                      </TableCell>
                      <TableCell>
                        {repository ?? (
                          <span className="text-fg-muted">{t("organization.applications.noRepository")}</span>
                        )}
                      </TableCell>
                      <TableCell align="right">
                        <Button
                          variant="secondary"
                          size="sm"
                          aria-label={t("appExport.labelFor", { name })}
                          loading={exporting === `${project}/${name}`}
                          disabled={repository === null}
                          disabledReason={repository === null ? t("appExport.noRepository") : undefined}
                          onClick={() => void exportOne(project, name)}
                        >
                          {t("appExport.button")}
                        </Button>
                      </TableCell>
                    </TableRow>
                  );
                })
              )}
            </TableBody>
          )}
        </Table>
      )}
    </section>
  );
}
