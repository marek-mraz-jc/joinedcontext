import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { projectDetailQuery } from "../../api/projects";
import { Button, Dialog, DialogClose, Field, InlineError, RadioGroup, Select } from "../ui";

/** `git` is one bundle per repository of a project in its own repository (MF-45). */
export type ExportFormat = "yaml" | "json" | "zip" | "git";

export interface ExportTarget {
  /** Plural of a single kind, or absent for the whole project. */
  plural?: string;
  /** One resource name, or absent for every resource of the selection. */
  name?: string;
}

/**
 * The query of one export, sent through the typed client to `/export` (UI-07). Built here so the
 * modal and its tests agree on it.
 *
 * It is fetched rather than followed as a link: a link hands the browser whatever comes back, so a
 * 403 or a 500 was saved as the export and the dialog closed on top of it (MF-16, T-1487).
 * ponytail: the archive is buffered in the browser; stream to disk when an export passes about
 * 100 MB.
 */
export function exportQuery(
  format: ExportFormat,
  target: ExportTarget,
  revision?: string,
): { format: string; kinds?: string; names?: string; revision?: string } {
  return {
    format,
    ...(target.plural ? { kinds: target.plural } : {}),
    ...(target.name ? { names: target.name } : {}),
    ...(revision ? { revision } : {}),
  };
}

/** The name the server gave the file, or the one this selection would have. */
export function filenameOf(
  answer: Response,
  project: string,
  format: ExportFormat,
  target: ExportTarget,
): string {
  const disposition = answer.headers.get("content-disposition") ?? "";
  const quoted = /filename\*?=(?:UTF-8'')?"?([^";]+)"?/i.exec(disposition);
  if (quoted?.[1]) {
    // A malformed escape is the server's slip, not a failed export: the name stays as it came.
    try {
      return decodeURIComponent(quoted[1]);
    } catch {
      return quoted[1];
    }
  }
  const extension = format === "zip" || format === "git" ? "zip" : format;
  return `${target.name ?? target.plural ?? project}.${extension}`;
}

/**
 * One-click download of a manifest, a bundle or the whole project archive, at the current
 * revision or an older one (MF-16, CC-49). No Git knowledge is asked of anyone: the revision
 * picker is the history rendered as sentences.
 *
 * From the project, the first and default choice is the whole project: the archive with every
 * manifest, the schema of every kind and data model, and a README saying what each file means
 * (MF-41). The plain YAML and JSON forms stay one click further, under other formats.
 */
export function ExportModal({
  project,
  target,
  open,
  onOpenChange,
}: {
  project: string;
  target: ExportTarget;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [format, setFormat] = useState<ExportFormat>(target.name ? "yaml" : "zip");
  const [revision, setRevision] = useState<string>("");
  const [refused, setRefused] = useState<string | null>(null);
  const [preparing, setPreparing] = useState(false);

  /** Fetch it, check the answer, then save it: a refusal keeps the dialog open with the reason. */
  async function download() {
    setRefused(null);
    setPreparing(true);
    try {
      // Through the typed client (UI-07): the route and its query are the API's own. A git
      // export is the repository at its default branch: it takes no revision (MF-45).
      const { data: blob, error, response: answer } = await api.GET("/api/v1/projects/{project}/export", {
        params: { path: { project }, query: exportQuery(format, target, format === "git" ? undefined : revision || undefined) },
        parseAs: "blob",
      });
      if (blob === undefined) {
        const problem = (error ?? {}) as { detail?: string; title?: string };
        // Not a problem document: the status is the whole of what the server said.
        setRefused(problem.detail ?? problem.title ?? (answer.statusText || `HTTP ${answer.status}`));
        return;
      }
      const href = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = href;
      link.download = filenameOf(answer, project, format, target);
      link.click();
      URL.revokeObjectURL(href);
      onOpenChange(false);
    } catch (error) {
      setRefused(error instanceof Error ? error.message : String(error));
    } finally {
      setPreparing(false);
    }
  }

  const revisions = useQuery({
    queryKey: ["revisions", project],
    enabled: open,
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/revisions", {
          params: { path: { project }, query: { limit: 20 } },
        }),
      ),
  });

  const whole = !target.name && !target.plural;
  // A project in a repository of its own (its registry entry names one, CC-85) also exports as
  // git, the form a move or a copy imports (MF-45).
  const detail = useQuery({ ...projectDetailQuery(project), enabled: open && whole });
  const spec = detail.data?.spec as { repository?: unknown } | null | undefined;
  const ownRepository = whole && Boolean(spec?.repository);
  // The whole project reads best as one archive, a single resource as one file; either way all
  // three are on screen. They used to be one visible radio and two behind a `details` nobody
  // opened, which is also how the arrow keys stopped walking the group.
  const formats: ExportFormat[] = whole
    ? ["zip", "yaml", "json", ...(ownRepository ? (["git"] as const) : [])]
    : ["yaml", "json", "zip"];

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      size="lg"
      title={t("export.title")}
      description={
        target.name
          ? t("export.hintResource", { name: target.name })
          : t("export.hintProject", { project })
      }
      closeLabel={t("form.cancel")}
      footer={
        <>
          <DialogClose asChild>
            <Button variant="secondary">{t("form.cancel")}</Button>
          </DialogClose>
          <Button
            variant="primary"
            loading={preparing}
            onClick={() => {
              void download();
            }}
          >
            {preparing ? t("export.preparing") : t("export.download")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <RadioGroup
          name="export-format"
          legend={t("export.format")}
          value={format}
          onChange={setFormat}
          options={formats.map((value) => ({
            value,
            label: value === "zip" && whole ? t("export.formats.whole") : t(`export.formats.${value}`),
            description:
              value === "zip" && whole
                ? t("export.formats.wholeHelp")
                : t(`export.formats.${value}Help`),
          }))}
        />

        {format === "git" ? null : (
          <Field
            id="export-revision"
            label={t("export.revision")}
            help={
              revisions.isError
                ? revisions.error instanceof ApiError && revisions.error.status === 503
                  ? t("export.noForge")
                  : t("export.noHistory")
                : undefined
            }
          >
            <Select
              id="export-revision"
              value={revision}
              onChange={(event) => setRevision(event.target.value)}
            >
              <option value="">{t("export.currentRevision")}</option>
              {(revisions.data?.items ?? []).map((commit) => (
                <option key={commit.sha} value={commit.sha}>
                  {`${commit.sha.slice(0, 7)} · ${commit.message} · ${
                    commit.date ? new Date(commit.date).toLocaleDateString(locale) : ""
                  }`}
                </option>
              ))}
            </Select>
          </Field>
        )}

        <p className="text-caption text-fg-muted">{t("export.secretsNote")}</p>
        {refused ? (
          <InlineError>
            {t("export.refused", { reason: refused })}
          </InlineError>
        ) : null}
      </div>
    </Dialog>
  );
}
