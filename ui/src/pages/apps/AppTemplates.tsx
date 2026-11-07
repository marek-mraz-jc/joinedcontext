import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { Alert, Badge, Button, Dialog, ExternalLink } from "../../components/ui";
import { AppGenerator } from "./AppGenerator";
import { UseTemplate } from "./UseTemplate";

type Template = components["schemas"]["Template"];

/** The prompt a template starts its run with: its purpose, naming it so the build adapts it. */
export function templatePrompt(template: Pick<Template, "name" | "purpose">): string {
  return `${template.purpose} (template: ${template.name})`;
}

/** The entity types a template reads, as its data needs name them. */
export function typesOf(template: { dataNeeds: readonly unknown[] }): string[] {
  return [
    ...new Set(
      template.dataNeeds.flatMap((need) => {
        const types = (need as { types?: unknown }).types;
        return Array.isArray(types) ? types.filter((type): type is string => typeof type === "string") : [];
      }),
    ),
  ];
}

/**
 * The App templates (T-3263, AP-141): what each is for, for whom and the data it needs, and
 * "create from this", which asks only which of the project's endpoints to read: the build adapts
 * the template to that data and the result runs as a preview before anyone edits it. "Use as is"
 * lands the template unchanged and asks no model (T-5617); while the model key is out it is the
 * one way in, and the gallery opens on it and says why.
 */
export function AppTemplates({ project, mayBuild }: { project: string; mayBuild: boolean }): JSX.Element | null {
  const { t } = useTranslation();
  const [chosen, setChosen] = useState<Template | null>(null);
  const [used, setUsed] = useState<Template | null>(null);
  const templates = useQuery({
    queryKey: ["app-templates"],
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/app-templates", {})),
  });
  const list = templates.data?.templates ?? [];
  if (list.length === 0) return null;
  const generates = templates.data?.modelAvailable !== false;
  return (
    <details className="rounded-lg border border-border p-3" data-testid="app-templates" open={!generates || undefined}>
      <summary className="cursor-pointer text-body font-semibold text-fg">{t("apps.templates.title")}</summary>
      <p className="mt-1 text-body text-fg-muted">{t("apps.templates.lead")}</p>
      {generates ? null : (
        <Alert tone="warning" className="mt-2">
          {t("apps.templates.modelOut")}
        </Alert>
      )}
      <ul className="mt-3 grid gap-3 md:grid-cols-2 xl:grid-cols-3">
        {list.map((template) => (
          <li key={template.name} className="flex flex-col gap-2 rounded-md border border-border bg-surface p-3">
            {/* The template as its demo shows it, taken by the SDK's demo journey (T-3306). */}
            <img
              src={`/api/v1/app-templates/${encodeURIComponent(template.name)}/screenshot/1440`}
              alt={t("apps.templates.screenshot", { title: template.title })}
              loading="lazy"
              className="aspect-[16/10] w-full rounded-sm border border-border object-cover object-top"
            />
            <h3 className="text-body font-semibold text-fg">{template.title}</h3>
            <p className="text-caption text-fg-muted">{template.purpose}</p>
            <p className="text-caption">
              <span className="font-semibold">{t("apps.templates.for")}</span> {template.audience}
            </p>
            <div className="flex flex-wrap items-center gap-1 text-caption">
              <span className="font-semibold">{t("apps.templates.needs")}</span>
              {typesOf(template).map((type) => (
                <Badge key={type} mono>
                  {type}
                </Badge>
              ))}
            </div>
            {template.access ? <p className="text-caption text-fg-muted">{template.access}</p> : null}
            <ExternalLink href={`/templates/${encodeURIComponent(template.name)}/`} className="w-fit text-caption">
              {t("apps.templates.demo")}
            </ExternalLink>
            <div className="mt-auto flex flex-wrap gap-2">
              <Button
                size="sm"
                variant="secondary"
                disabled={!mayBuild || !generates}
                disabledReason={!mayBuild ? t("apps.templates.mayNot") : generates ? undefined : t("apps.templates.modelOutShort")}
                onClick={() => setChosen(template)}
              >
                {t("apps.templates.create")}
              </Button>
              <Button
                size="sm"
                variant={generates ? "ghost" : "primary"}
                disabled={!mayBuild}
                disabledReason={mayBuild ? undefined : t("apps.templates.mayNot")}
                onClick={() => setUsed(template)}
              >
                {t("apps.templates.useAsIs")}
              </Button>
            </div>
          </li>
        ))}
      </ul>
      <Dialog
        open={chosen !== null}
        onOpenChange={(open) => (open ? undefined : setChosen(null))}
        title={chosen ? t("apps.templates.createTitle", { title: chosen.title }) : ""}
        description={t("apps.templates.createHint")}
        closeLabel={t("app.close")}
        size="lg"
      >
        {chosen ? <AppGenerator project={project} initialName={chosen.name} initialPrompt={templatePrompt(chosen)} /> : null}
      </Dialog>
      <Dialog
        open={used !== null}
        onOpenChange={(open) => (open ? undefined : setUsed(null))}
        title={used ? t("apps.templates.useTitle", { title: used.title }) : ""}
        description={t("apps.templates.useHint")}
        closeLabel={t("app.close")}
      >
        {used ? <UseTemplate project={project} template={used} /> : null}
      </Dialog>
    </details>
  );
}
