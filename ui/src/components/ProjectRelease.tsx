import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../api/client";
import type { Change } from "../api/manifest";
import { ORG_NAMESPACE } from "../api/manifest";
import { usePermissions } from "../api/permissions";
import { ChangeNotice } from "./ChangeNotice";
import { numberProblems, ParameterFields, parameterValues } from "./ImportProjectDialog";
import type { Declaration } from "./ImportProjectDialog";
import { Alert, Button, Dialog, Field, PageFailed, Select } from "./ui";

/** What `GET …/registry` answers (PF-86, CC-88). */
interface Entry {
  repository: { name?: string; url?: string };
  ref: string;
  parameters: Record<string, string | number | boolean>;
  declarations: Record<string, Declaration>;
  tags: { name: string; commit: string }[];
}

const registryKey = (project: string) => ["projects", project, "registry"] as const;

function failureOf(error: unknown, fallback: string): string | null {
  if (error instanceof ApiError) {
    return error.problem?.detail ?? error.message;
  }
  return error ? fallback : null;
}

/**
 * Pinning a release (PF-86, CC-88, T-3432): the ref to run, offered from the repository's tags,
 * and this deployment's values, drawn from the declarations; then one
 * `PUT /api/v1/projects/{project}/registry`, which proposes the registry entry as a red-lane
 * organization change.
 */
function PinReleaseDialog({
  project,
  entry,
  open,
  onOpenChange,
}: {
  project: string;
  entry: Entry;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const queryClient = useQueryClient();
  const current = () =>
    Object.fromEntries(Object.entries(entry.parameters).map(([name, value]) => [name, String(value)]));
  const [ref, setRef] = useState(entry.ref);
  const [typed, setTyped] = useState<Record<string, string>>(current);
  const [change, setChange] = useState<Change | null>(null);
  const refs = [entry.ref, ...entry.tags.map((tag) => tag.name).filter((name) => name !== entry.ref)];

  const pin = useMutation({
    mutationFn: async () => {
      const wrong = numberProblems(entry.declarations, typed);
      if (wrong.length > 0) {
        throw new Error(t("projectImport.notNumber", { names: wrong.join(", ") }));
      }
      return unwrap(
        await api.PUT("/api/v1/projects/{project}/registry", {
          params: { path: { project } },
          body: { ref, parameters: parameterValues(entry.declarations, typed) },
        }),
      );
    },
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: registryKey(project) });
      setChange(result);
    },
  });

  const close = (next: boolean) => {
    if (!next) {
      setRef(entry.ref);
      setTyped(current());
      setChange(null);
      pin.reset();
    }
    onOpenChange(next);
  };
  const failure = failureOf(pin.error, t("app.error.generic"));
  const declarations = Object.entries(entry.declarations);

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      title={t("projectRelease.dialogTitle", { name: project })}
      description={t("projectRelease.dialogLead")}
      closeLabel={t("resourceDelete.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            <Button variant="primary" loading={pin.isPending} onClick={() => pin.mutate()}>
              {t("projectRelease.propose")}
            </Button>
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
          <Field id={`${ids}-ref`} label={t("projectRelease.ref")} help={t("projectRelease.refHint")} required>
            <Select id={`${ids}-ref`} value={ref} onChange={(event) => setRef(event.target.value)}>
              {refs.map((name) => (
                <option key={name} value={name}>
                  {name === entry.ref ? t("projectRelease.current", { ref: name }) : name}
                </option>
              ))}
            </Select>
          </Field>
          <h3 className="text-body font-semibold text-fg">{t("projectImport.parameters")}</h3>
          {declarations.length === 0 ? (
            <p className="text-body text-fg-muted">{t("projectImport.noParameters")}</p>
          ) : (
            <ParameterFields
              idPrefix={`${ids}-param`}
              declarations={declarations}
              typed={typed}
              onChange={setTyped}
            />
          )}
        </div>
      )}
    </Dialog>
  );
}

/**
 * Release (PF-86, CC-88): the ref of the project's repository this deployment runs and its
 * parameter values, as the registry entry says, with the control that proposes another. Only a
 * project in a repository of its own has one; the control is refused with the reason, never
 * hidden, for a person who may not propose here (UI-44).
 */
export function ProjectRelease({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const mayPropose = usePermissions(ORG_NAMESPACE).can("Project", "propose");
  const entry = useQuery({
    queryKey: registryKey(project),
    queryFn: async (): Promise<Entry> =>
      (await unwrap(
        await api.GET("/api/v1/projects/{project}/registry", {
          params: { path: { project } },
        }),
      )) as Entry,
  });
  const values = Object.entries(entry.data?.parameters ?? {});
  const external = entry.data !== undefined && entry.data.repository.name === undefined;

  return (
    <section className="space-y-4" aria-labelledby="project-release-heading">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 id="project-release-heading" className="text-title font-semibold text-fg">
            {t("projectRelease.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("projectRelease.lead")}</p>
        </div>
        {entry.data ? (
          <Button
            size="sm"
            variant="secondary"
            disabled={!mayPropose || external}
            disabledReason={
              external ? t("projectRelease.external") : mayPropose ? undefined : t("projectRelease.needsPropose")
            }
            onClick={() => setOpen(true)}
          >
            {t("projectRelease.button")}
          </Button>
        ) : null}
      </div>
      {entry.isError ? (
        <PageFailed
          error={entry.error}
          onRetry={() => {
            void entry.refetch();
          }}
        />
      ) : (
        <dl className="grid gap-x-6 gap-y-3 text-body sm:grid-cols-[max-content_1fr]">
          <dt className="font-medium text-fg">{t("projectRelease.runs")}</dt>
          <dd className="font-mono text-fg">{entry.isPending ? t("app.loading") : entry.data.ref}</dd>
          <dt className="font-medium text-fg">{t("projectRelease.parameters")}</dt>
          <dd className="text-fg">
            {entry.isPending ? (
              t("app.loading")
            ) : values.length === 0 ? (
              t("projectRelease.none")
            ) : (
              <ul>
                {values.map(([name, value]) => (
                  <li key={name}>
                    <span className="font-mono">{name}</span>: {String(value)}
                  </li>
                ))}
              </ul>
            )}
          </dd>
        </dl>
      )}
      {entry.data && !external ? (
        <PinReleaseDialog project={project} entry={entry.data} open={open} onOpenChange={setOpen} />
      ) : null}
    </section>
  );
}
