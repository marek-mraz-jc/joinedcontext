import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { isChange, localized } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { SchemaForm } from "../../components/forms/SchemaForm";
import { pickers } from "../../components/forms/hints";
import type { JsonSchema } from "../../components/forms/types";
import { blueprintSpec } from "./Gallery";
import { Alert, Button, PageHeader } from "../../components/ui";

type Parameters = Record<string, unknown>;

/**
 * The instantiation wizard: one form, generated from the blueprint's own parameter schema
 * (CC-24, CC-31), never hand-built per blueprint. Saving proposes a change like every other
 * write in the Portal, so the user never sees a branch or a merge request (CC-29, CC-32).
 *
 * The same schema validates in the browser and on the server. The browser copy is what makes
 * the form usable; the server copy is the one that decides (CC-60).
 */
export function Instantiate({
  project,
  blueprint,
  onBack,
  backLabel,
}: {
  project: string;
  blueprint: Manifest;
  onBack: () => void;
  /** Names the page the person goes back to. */
  backLabel: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** One entry per rule the parameters broke, so every bad field is named at once (CC-24). */
  const [violations, setViolations] = useState<string[]>([]);

  const spec = blueprintSpec(blueprint);
  const name = localized(blueprint.metadata.title, i18n.language, blueprint.metadata.name);
  const schema = useMemo(
    () => inLanguage(spec.parameterSchema, i18n.language) as JsonSchema | undefined,
    [spec.parameterSchema, i18n.language],
  );
  // A parameter may ask for a picker over the platform's own state rather than a fixed enum
  // (CC-24). The project comes from the page, never from the blueprint.
  const uiSchema = useMemo(() => pickers(schema, { project }), [schema, project]);

  const start = useMutation({
    mutationFn: async (parameters: Parameters) => {
      setError(null);
      setViolations([]);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/flows", {
          params: { path: { project } },
          body: {
            blueprint: blueprint.metadata.name,
            // The version the form was generated from: expanding these values against a
            // schema the user never saw is how a form produces a manifest nobody reviewed.
            version: spec.version ?? "",
            parameters,
          },
        }),
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
    },
    onError: (err) => {
      setError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("flows.instantiate.failed"),
      );
      setViolations(err instanceof ApiError ? (err.problem?.errors ?? []) : []);
    },
  });

  return (
    <div className="space-y-4">
      <Button size="sm" onClick={onBack}>
        {backLabel}
      </Button>

      <PageHeader
        title={t("flows.instantiate.title", { name })}
        description={localized(blueprint.metadata.description, i18n.language, "")}
      />

      {change && <ChangeNotice change={change} project={project} />}
      {error && (
        <Alert role="alert" tone="danger">
          <p>{error}</p>
          {violations.length > 0 && (
            <ul className="mt-1 list-disc pl-5 text-caption">
              {violations.map((violation) => (
                <li key={violation}>{violation}</li>
              ))}
            </ul>
          )}
        </Alert>
      )}

      {schema ? (
        <>
          <SchemaForm<Parameters>
            schema={schema}
            uiSchema={uiSchema}
            // Proposed once: a second press would open a second Change for the same set-up.
            disabled={start.isPending || change !== null}
            submitDisabledReason={change ? t("flows.instantiate.proposed") : undefined}
            // `disabled` alone dims the button and says nothing, and somebody who pressed it and
            // saw nothing move pressed it again; `submitting` keeps the label and spins (UI-01).
            submitting={start.isPending}
            submitLabel={t("flows.instantiate.submit")}
            onSubmit={(parameters) => {
              start.mutate(parameters);
            }}
          />
          <p className="text-body text-fg-muted">{t("flows.instantiate.hint")}</p>
        </>
      ) : (
        // A blueprint with no parameters still has to be startable: the page used to end here
        // with a sentence and nothing to press, so a card offered in the gallery could not be
        // set up at all from the Portal.
        <div className="space-y-3">
          <p className="text-body text-fg-muted">{t("flows.instantiate.noSchema")}</p>
          <Button
            variant="primary"
            loading={start.isPending}
            disabled={change !== null}
            disabledReason={change ? t("flows.instantiate.proposed") : undefined}
            onClick={() => {
              start.mutate({});
            }}
          >
            {t("flows.instantiate.submit")}
          </Button>
          <p className="text-body text-fg-muted">{t("flows.instantiate.hint")}</p>
        </div>
      )}
    </div>
  );
}

/**
 * The parameter schema in `locale` (T-4244): a `title` or `description` gives way to the entry of
 * the `x-jc-title`/`x-jc-description` map beside it, at every depth, `oneOf` choices included.
 * Without an entry the plain string stands. Values (`const`, `default`, `enum`, `examples`) are
 * what gets submitted and stay as written.
 */
export function inLanguage(node: unknown, locale: string): unknown {
  if (Array.isArray(node)) return node.map((item) => inLanguage(item, locale));
  if (!node || typeof node !== "object") return node;
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(node)) {
    out[key] = ["const", "default", "enum", "examples"].includes(key) ? value : inLanguage(value, locale);
  }
  for (const key of ["title", "description"]) {
    const texts = out[`x-jc-${key}`];
    if (!texts || typeof texts !== "object" || Array.isArray(texts)) continue;
    const text = (texts as Record<string, unknown>)[locale] ?? (texts as Record<string, unknown>)[locale.split("-")[0]];
    if (typeof text === "string" && text.trim() !== "") out[key] = text;
  }
  return out;
}
