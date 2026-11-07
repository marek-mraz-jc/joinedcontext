/**
 * The pipelines a project saved to start from (T-3976, API/01 §42): listed in the gallery beside
 * the recipes, and saved from the editor. A template is the Portal's record of a pipeline's shape
 * without its credentials and its references into the project, so using one fills the form and
 * the person picks the source and the target again before anything is checked and proposed.
 */
import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, reasonOf } from "../../api/client";
import type { Manifest } from "../../api/manifest";
import { usePermissions } from "../../api/permissions";
import { Alert, Button, Field, Input } from "../../components/ui";
import { toForm } from "./PipelineEditor";
import type { PipelineForm } from "./PipelineEditor";

const key = (project: string) => [...queryKeys.resource(project, "pipelines", ""), "templates"] as const;

/** A template's spec as the form holds it: no name, the source and target left to pick. */
export function formOfTemplate(spec: Record<string, unknown>): PipelineForm {
  const { name: _name, ...form } = toForm({
    apiVersion: "joinedcontext.com/v1alpha2",
    kind: "Pipeline",
    metadata: { name: "" },
    spec,
  } as Manifest);
  void _name;
  return form;
}

export function TemplateList({ project, onUse }: { project: string; onUse: (form: PipelineForm) => void }): JSX.Element | null {
  const { t } = useTranslation();
  const { can } = usePermissions(project);
  const queryClient = useQueryClient();
  const templates = useQuery({
    queryKey: key(project),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/pipelinetemplates", { params: { path: { project } } })),
  });
  const remove = useMutation({
    mutationFn: async (id: string) =>
      unwrap(
        await api.DELETE("/api/v1/projects/{project}/pipelinetemplates/{id}", {
          params: { path: { project, id } },
        }),
      ),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: key(project) }),
  });
  const items = templates.data?.items ?? [];
  if (items.length === 0) return null;
  return (
    <div className="mt-3 flex flex-col gap-2" data-testid="pipeline-templates">
      <h3 className="text-caption font-semibold text-fg">{t("pipelines.templates.title")}</h3>
      {remove.error ? (
        <Alert tone="danger" role="alert">
          {reasonOf(remove.error, t("app.error.generic"))}
        </Alert>
      ) : null}
      <ul className="grid gap-2 sm:grid-cols-2" aria-label={t("pipelines.templates.title")}>
        {items.map((template) => (
          <li
            key={template.id}
            className="flex flex-col gap-1 rounded-md border border-border bg-surface p-2"
            data-testid="pipeline-template"
          >
            <span className="text-body font-medium text-fg">{template.title}</span>
            <span className="text-caption text-fg-muted">{t("pipelines.templates.by", { name: template.createdBy })}</span>
            <div className="flex gap-2">
              <Button size="sm" variant="secondary" onClick={() => onUse(formOfTemplate(template.spec as Record<string, unknown>))}>
                {t("pipelines.templates.use")}
              </Button>
              {can("Pipeline", "propose") ? (
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={t("pipelines.templates.deleteOf", { title: template.title })}
                  loading={remove.isPending && remove.variables === template.id}
                  onClick={() => remove.mutate(template.id)}
                >
                  {t("pipelines.templates.delete")}
                </Button>
              ) : null}
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** "Save as template" under the editor, for a person who may propose pipelines. */
export function SaveAsTemplate({ project, manifest }: { project: string; manifest: () => Manifest }): JSX.Element | null {
  const { t } = useTranslation();
  const { can } = usePermissions(project);
  const queryClient = useQueryClient();
  const [title, setTitle] = useState<string | null>(null);
  const save = useMutation({
    mutationFn: async (name: string) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/pipelinetemplates", {
          params: { path: { project } },
          // A free-form object in the published schema, typed as `Record<string, never>` there.
          body: { title: name, manifest: manifest() as unknown as Record<string, never> },
        }),
      ),
    onSuccess: () => {
      setTitle(null);
      void queryClient.invalidateQueries({ queryKey: key(project) });
    },
  });
  if (!can("Pipeline", "propose")) return null;
  return (
    <div className="flex flex-col gap-2" data-testid="save-as-template">
      {title === null ? (
        <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" variant="secondary" onClick={() => setTitle("")}>
            {t("pipelines.templates.save")}
          </Button>
          {save.data ? (
            <span role="status" className="text-caption text-fg-muted">
              {t("pipelines.templates.saved", { title: save.data.title })}
            </span>
          ) : null}
        </div>
      ) : (
        <div className="flex flex-wrap items-end gap-2">
          <Field id="template-title" label={t("pipelines.templates.titleField")} help={t("pipelines.templates.titleHelp")} required>
            <Input id="template-title" value={title} maxLength={120} onChange={(event) => setTitle(event.target.value)} />
          </Field>
          <Button
            size="sm"
            variant="primary"
            disabled={!title.trim()}
            disabledReason={t("pipelines.templates.titleNeeded")}
            loading={save.isPending}
            onClick={() => save.mutate(title.trim())}
          >
            {t("pipelines.templates.saveIt")}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setTitle(null)}>
            {t("form.cancel")}
          </Button>
        </div>
      )}
      {save.error ? (
        <Alert tone="danger" role="alert">
          {reasonOf(save.error, t("app.error.generic"))}
        </Alert>
      ) : null}
    </div>
  );
}
