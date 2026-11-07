/**
 * One published model, read-only (DM-61, T-2762, T-2765): the entity form each class generates
 * filled with the generated example, the LinkML itself, and the sources of the models it imports
 * for the model page's diagram.
 *
 * Nothing here edits: Edit is the editor's (`/projects/{project}/models?edit=<name>`), and a view
 * that also wrote would be a second writer of the same document. The YAML is shown as text, never as HTML,
 * and needs no Monaco: a read-only page stays light until the person opens the editor.
 */
import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { readModelSource } from "../../api/datamodelSource";
import { SchemaForm } from "../../components/forms/SchemaForm";
import type { JsonSchema } from "../../components/forms/types";
import { CodeBlock, Alert, Button, Field, Select, Skeleton } from "../../components/ui";
import { download } from "./diagramExport";
import type { Artifacts } from "./LinkmlPreviewPanel";
import { formReady } from "./formSchema";
import { importName, parseModel } from "./linkml";
import { useProjectList } from "./ModelsList";

/** The artifacts of one source, compiled by Model Tools through the Portal (DM-17). */
export function useArtifacts(source: string | undefined) {
  return useQuery({
    queryKey: ["tools", "generate", source],
    enabled: source !== undefined && source.trim() !== "",
    // A preview is a function of the source: the editor's cache answers it too.
    staleTime: Infinity,
    retry: false,
    queryFn: async (): Promise<Artifacts> =>
      unwrap(await api.POST("/api/v1/tools/generate", { body: { source: source ?? "" } })) as Artifacts,
  });
}

/**
 * The sources of the project's models this one imports, by name (T-2720): what the diagram needs
 * to draw an imported class beside the model's own. An import that names no model of the project
 * (`linkml:types`, a URL elsewhere) or one the person cannot read is left out, and the model is
 * drawn without it.
 */
export function useImportSources(project: string, name: string, source: string): Record<string, string> {
  const wanted = useMemo(
    () =>
      new Set(
        (parseModel(source).imports ?? [])
          .map(importName)
          .filter((one): one is string => one !== undefined && one !== name),
      ),
    [source, name],
  );
  const models = useProjectList(project, "datamodels").data ?? [];
  const imported = models.filter((model) => wanted.has(model.metadata.name));
  const texts = useQueries({
    queries: imported.map((model) => {
      const inline = typeof model.spec.linkml === "string" && model.spec.linkml.includes("\n") ? model.spec.linkml : undefined;
      return {
        queryKey: ["datamodel-source", project, model.metadata.name],
        retry: false,
        queryFn: () => (inline !== undefined ? Promise.resolve(inline) : readModelSource(project, model.metadata.name)),
      };
    }),
  }).map((query) => query.data);
  const names = imported.map((model) => model.metadata.name);
  const key = JSON.stringify([names, texts]);
  // One object while nothing changed, so the diagram is drawn once.
  return useMemo(() => {
    const [keyNames, keyTexts] = JSON.parse(key) as [string[], (string | undefined)[]];
    return Object.fromEntries(
      keyNames.flatMap((one, index) => (typeof keyTexts[index] === "string" ? [[one, keyTexts[index]]] : [])),
    );
  }, [key]);
}

/**
 * The schema of one class as a form reads it: its own definition, with the document's
 * definitions beside it so a `$ref` to an enum or another class still resolves.
 */
export function classSchema(jsonSchema: Record<string, unknown> | undefined, name: string): JsonSchema | undefined {
  const definitions = (jsonSchema?.definitions ?? jsonSchema?.$defs) as Record<string, unknown> | undefined;
  const own = definitions?.[name];
  if (typeof own !== "object" || own === null) {
    return undefined;
  }
  return { ...(own as JsonSchema), definitions } as JsonSchema;
}

/** The generated example when it is an entity of `name` (or names no type), for the form to show. */
function exampleOf(example: Record<string, unknown> | undefined, name: string): Record<string, unknown> | undefined {
  if (!example) {
    return undefined;
  }
  return example.type === undefined || example.type === name ? example : undefined;
}

/** The form each class generates, one class at a time (DM-20). */
export function ModelForm({ source, initialClass }: { source: string; initialClass?: string }): JSX.Element {
  const { t } = useTranslation();
  const classes = useMemo(() => parseModel(source).classes.map((one) => one.name), [source]);
  const [chosen, setChosen] = useState(initialClass ?? "");
  const current = classes.includes(chosen) ? chosen : (classes[0] ?? "");
  const artifacts = useArtifacts(source);

  if (classes.length === 0) {
    return <p className="text-body text-fg-muted">{t("models.page.noClasses")}</p>;
  }
  const generated = classSchema(artifacts.data?.jsonSchema, current);
  const form = generated
    ? formReady(
        generated as Record<string, unknown>,
        exampleOf(artifacts.data?.example, current),
        (type) => t(`models.page.jsonType.${type}`, { defaultValue: type }),
        t("models.page.asJson"),
      )
    : undefined;
  const failed =
    artifacts.error instanceof ApiError ? (artifacts.error.problem?.detail ?? artifacts.error.message) : null;
  return (
    <div className="flex flex-col gap-3">
      <Field id="model-form-class" label={t("models.page.formClass")} className="w-fit">
        <Select id="model-form-class" value={current} onChange={(event) => setChosen(event.target.value)}>
          {classes.map((one) => (
            <option key={one} value={one}>
              {one}
            </option>
          ))}
        </Select>
      </Field>
      {artifacts.isPending ? (
        <div role="status" aria-label={t("models.previewCompiling")} className="flex flex-col gap-2">
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      ) : artifacts.isError ? (
        <Alert tone="warning" role="status">
          <p>
            {t("models.previewUnavailable")}
            {failed ? ` ${failed}` : ""}
          </p>
          <Button size="sm" className="mt-2" onClick={() => void artifacts.refetch()}>
            {t("app.error.retry")}
          </Button>
        </Alert>
      ) : form ? (
        <SchemaForm
          key={current}
          schema={form.schema as JsonSchema}
          formData={form.data as Record<string, unknown> | undefined}
          disabled
          onSubmit={() => undefined}
          submitLabel={t("models.formPreview")}
          submitDisabledReason={t("models.page.formReadOnly")}
        />
      ) : (
        <p className="text-body text-fg-muted">{t("models.noArtifact")}</p>
      )}
    </div>
  );
}

/** The parts of one YAML line a reader tells apart: indentation, key, value, comment. */
export function yamlParts(line: string): { indent: string; key?: string; rest: string; comment?: string } {
  const indent = /^\s*(- )?/.exec(line)?.[0] ?? "";
  const body = line.slice(indent.length);
  if (body.startsWith("#")) {
    return { indent, rest: "", comment: body };
  }
  // A key is the text before the first `: ` or a trailing `:`, outside quotes.
  const key = /^([^'"#:\s][^:#]*?|"[^"]*"|'[^']*'):(?=\s|$)/.exec(body)?.[0];
  const after = key ? body.slice(key.length) : body;
  const hash = after.search(/\s#/);
  return hash >= 0
    ? { indent, key, rest: after.slice(0, hash), comment: after.slice(hash) }
    : { indent, key, rest: after };
}

/** The LinkML as text, keys and comments set apart, with copy and download. */
export function ModelYaml({ source, name }: { source: string; name: string }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState<"yes" | "no" | null>(null);
  const lines = useMemo(() => source.replace(/\n$/, "").split("\n"), [source]);

  const copy = () => {
    const clipboard = navigator.clipboard as Clipboard | undefined;
    if (!clipboard) {
      setCopied("no");
      return;
    }
    clipboard.writeText(source).then(
      () => setCopied("yes"),
      () => setCopied("no"),
    );
  };
  const downloadYaml = () => download(source, `${name}.linkml.yaml`, "application/yaml");

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={copy}>
          {t("models.page.copyYaml")}
        </Button>
        <Button size="sm" variant="secondary" onClick={downloadYaml}>
          {t("models.page.downloadYaml")}
        </Button>
        <span role="status" className="text-caption text-fg-muted">
          {copied === "yes" ? t("models.page.copied") : copied === "no" ? t("models.page.copyFailed") : ""}
        </span>
      </div>
      <CodeBlock
        label={t("models.page.yaml")}
        // Down to near the page bottom: a model is any length (owner 2026-10-31).
        className="h-[calc(100dvh-15rem)] min-h-64 rounded border border-border bg-surface-subtle p-3 font-mono text-caption text-fg"
      >
        {lines.map((line, index) => {
          const parts = yamlParts(line);
          return (
            // A long line wraps inside the box under its own key, so nothing runs past the right
            // border or needs a sideways scroll on a phone (T-6254 judge); the indent never wraps.
            <span key={index} className="flex">
              <span className="shrink-0 whitespace-pre">{parts.indent}</span>
              <span className="min-w-0 whitespace-pre-wrap break-words">
                {parts.key ? <span className="text-primary-soft-fg">{parts.key}</span> : null}
                {parts.rest}
                {parts.comment ? <span className="text-fg-muted italic">{parts.comment}</span> : null}
              </span>
            </span>
          );
        })}
      </CodeBlock>
    </div>
  );
}
