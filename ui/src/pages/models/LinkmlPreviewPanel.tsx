import { useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { SchemaForm } from "../../components/forms/SchemaForm";
import type { JsonSchema } from "../../components/forms/types";
import { CodeBlock,
  Alert,
  Button,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Tabs,
  tabPanelProps,
} from "../../components/ui";
import {
  DEFAULT_KIND,
  parseModel,
  slotAffordance,
  slotDimension,
  unitCode,
} from "./linkml";
import type { LinkmlSlot } from "./linkml";

/**
 * What the model compiles to, refreshed while the user types (DM-17).
 *
 * Every artifact comes from Model Tools through the Portal: the browser never compiles a
 * schema itself and never reaches a third party (DM-10, DM-18). A source that does not compile
 * yet answers with messages and no artifacts, which is the normal state of an editor, so the
 * panel keeps the last good preview visible next to the messages instead of blanking.
 */
export interface LinkmlPreviewPanelProps {
  source: string;
  /** How long to wait after the last keystroke before compiling. */
  debounceMs?: number;
}

export interface Artifacts {
  /** The LinkML source itself; an import answers with it, a compilation does not (DM-07). */
  linkml?: string;
  jsonSchema?: Record<string, unknown>;
  context?: Record<string, unknown>;
  /** The documentation page Model Tools renders, which is the one the repository commits. */
  docs?: string;
  shacl?: string;
  owl?: string;
  example?: Record<string, unknown>;
  generatorVersion?: string;
  errors?: string[];
}

type Tab = "schema" | "context" | "example" | "entity" | "form" | "options" | "docs";

const TABS: Tab[] = ["schema", "context", "example", "entity", "form", "options", "docs"];

/** The JSON-LD keywords an example carries that no `@context` has to define. */
const KEYWORDS = ["id", "type", "@context", "@id", "@type"];

/**
 * The example as an endpoint serves it: one NGSI-LD entity in normalized form (T-1092).
 *
 * The generator's example is keyValues — `pm10: 31.4` — which is not what a consumer parses.
 * Each attribute is wrapped in the member its slot declares, so the author sees the payload the
 * space will actually carry and can tell a Property from a Relationship before publishing.
 */
export function normalizedEntity(
  example: Record<string, unknown> | undefined,
  slots: LinkmlSlot[],
): Record<string, unknown> | undefined {
  if (example === undefined) {
    return undefined;
  }
  const kinds = new Map(slots.map((slot) => [slot.name, slot.kind]));
  const units = new Map(slots.map((slot) => [slot.name, unitCode(slot.unit)]));
  const entity: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(example)) {
    if (KEYWORDS.includes(name)) {
      entity[name] = value;
      continue;
    }
    const kind = kinds.get(name) ?? DEFAULT_KIND;
    const member =
      kind === "Relationship"
        ? { type: kind, object: value }
        : kind === "LanguageProperty"
          ? { type: kind, languageMap: value }
          : kind === "JsonProperty"
            ? { type: kind, json: value }
            : kind === "VocabProperty"
              ? { type: kind, vocab: value }
              : { type: kind, value };
    const unit = units.get(name);
    entity[name] = unit === undefined ? member : { ...member, unitCode: unit };
  }
  return entity;
}

/**
 * The attributes of the example that the generated `@context` does not define (DM-21).
 *
 * An example that expands with warnings is an example that would federate as opaque data, so
 * the panel says which term is missing rather than showing a green preview of a broken model.
 */
export function unmappedTerms(
  example: Record<string, unknown> | undefined,
  context: Record<string, unknown> | undefined,
): string[] {
  if (!example) {
    return [];
  }
  // A `@context` is one object, or an array of objects and URLs of more (JSON-LD 1.1 §4.1).
  const inner = context?.["@context"] ?? context;
  const parts = Array.isArray(inner) ? inner : [inner];
  const defined = new Set<string>();
  let certain = true;
  for (const part of parts) {
    if (typeof part === "string") {
      // A context this panel cannot fetch may define any of these terms (T-1096). Naming one
      // unmapped would be a false alarm, and a false alarm about a model is worse than silence.
      certain = false;
      continue;
    }
    if (part === null || typeof part !== "object") {
      continue;
    }
    const entries = part as Record<string, unknown>;
    if (entries["@vocab"] !== undefined) {
      // Every term the example carries resolves under the vocabulary, so none is unmapped.
      return [];
    }
    for (const term of Object.keys(entries)) {
      defined.add(term);
    }
  }
  if (!certain) {
    return [];
  }
  return Object.keys(example).filter(
    (term) => !KEYWORDS.includes(term) && !defined.has(term) && !term.startsWith("@"),
  );
}

function Json({ value }: { value: unknown }): JSX.Element {
  const { t } = useTranslation();
  return (
    <CodeBlock label={t("codeBlock.json")} className="max-h-96 rounded border border-border bg-surface-subtle p-3 font-mono text-caption text-fg">
      {JSON.stringify(value, null, 2)}
    </CodeBlock>
  );
}

/** The shape of an artifact while Model Tools is compiling it, in its place and its size. */
function PreviewSkeleton({ label }: { label: string }): JSX.Element {
  return (
    <div
      role="status"
      aria-label={label}
      className="flex flex-col gap-2 rounded border border-border bg-surface-subtle p-3"
    >
      <Skeleton className="h-4 w-2/3" />
      <Skeleton className="h-4 w-1/2" />
      <Skeleton className="h-4 w-3/4" />
      <Skeleton className="h-4 w-1/3" />
    </div>
  );
}

export function LinkmlPreviewPanel({
  source,
  debounceMs = 400,
}: LinkmlPreviewPanelProps): JSX.Element {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("schema");
  const [settled, setSettled] = useState(source);

  // The editor asks on every pause in typing, not on every keystroke: Model Tools is shared
  // and a compile per character would queue behind itself.
  useEffect(() => {
    const timer = setTimeout(() => setSettled(source), debounceMs);
    return () => clearTimeout(timer);
  }, [source, debounceMs]);

  const preview = useQuery({
    queryKey: ["tools", "generate", settled],
    enabled: settled.trim().length > 0,
    // A preview is a function of the source, so a source seen before never needs recompiling.
    staleTime: Infinity,
    retry: false,
    queryFn: async (): Promise<Artifacts> =>
      unwrap(
        await api.POST("/api/v1/tools/generate", {
          body: { source: settled },
        }),
      ) as Artifacts,
  });

  const model = useMemo(() => parseModel(settled), [settled]);
  // DM-20: an enum range is a select, which needs the model's own enum names to recognise.
  const enumNames = useMemo(() => model.enums.map((one) => one.name), [model]);
  const artifacts = preview.data;
  const waiting = settled.trim().length === 0;
  const compiling = !waiting && preview.isPending;
  // An artifact this compilation did not produce — the compile failed, or the model has no
  // example — is said in words. `JSON.stringify(undefined ?? null)` printed `null` in a code
  // box, which reads as an answer rather than as its absence.
  const artifact = (value: unknown): JSX.Element =>
    value === undefined ? (
      <p className="text-body text-fg-muted">{t("models.noArtifact")}</p>
    ) : (
      <Json value={value} />
    );
  const detail =
    preview.error instanceof ApiError ? (preview.error.problem?.detail ?? preview.error.message) : null;
  const missing = useMemo(
    () => unmappedTerms(artifacts?.example, artifacts?.context),
    [artifacts],
  );

  return (
    <div className="flex flex-col gap-3">
      <Tabs
        id="linkml-preview"
        variant="pill"
        label={t("models.preview")}
        tabs={TABS.map((name) => ({ value: name, label: t(`models.tab.${name}`) }))}
        value={tab}
        onChange={setTab}
      />

      {preview.isError ? (
        <Alert role="status" tone="warning">
          <p>
            {t("models.previewUnavailable")}
            {detail ? ` ${detail}` : ""}
          </p>
          <Button
            size="sm"
            className="mt-2"
            onClick={() => {
              void preview.refetch();
            }}
          >
            {t("app.error.retry")}
          </Button>
        </Alert>
      ) : null}
      {artifacts?.errors && artifacts.errors.length > 0 ? (
        <Alert role="status" tone="warning">
          <ul className="flex flex-col gap-1">
            {artifacts.errors.map((message) => (
              <li key={message}>{message}</li>
            ))}
          </ul>
        </Alert>
      ) : null}
      {missing.length > 0 ? (
        <Alert role="alert" tone="danger">
          {t("models.exampleUnmapped", { terms: missing.join(", ") })}
        </Alert>
      ) : null}

      <div {...tabPanelProps("linkml-preview", tab)}>
        {/*
          Nothing to compile, compiling, and compiled are three different things, and the panel
          used to draw the first two the same way: `JSON.stringify(undefined ?? null)` — the word
          `null` in a code box, which reads as an artifact that came back empty.
        */}
        {waiting ? (
          <p className="text-body text-fg-muted">{t("models.previewEmpty")}</p>
        ) : compiling ? (
          <PreviewSkeleton label={t("models.previewCompiling")} />
        ) : (
          <>
            {tab === "schema" ? artifact(artifacts?.jsonSchema) : null}
            {tab === "context" ? artifact(artifacts?.context) : null}
            {tab === "example" ? artifact(artifacts?.example) : null}
            {tab === "entity" ? artifact(normalizedEntity(artifacts?.example, model.slots)) : null}
            {tab === "form" ? (
              artifacts?.jsonSchema ? (
                <SchemaForm
                  schema={artifacts.jsonSchema as JsonSchema}
                  formData={artifacts.example}
                  disabled
                  onSubmit={() => undefined}
                  submitLabel={t("models.formPreview")}
                />
              ) : (
                <p className="text-body text-fg-muted">{t("models.noArtifact")}</p>
              )
            ) : null}
            {tab === "options" ? (
              <Table caption={t("models.tab.options")}>
                <TableHead>
                  <TableHeaderCell>{t("models.slot")}</TableHeaderCell>
                  <TableHeaderCell>{t("models.affordanceLabel")}</TableHeaderCell>
                  <TableHeaderCell>{t("models.dimensionLabel")}</TableHeaderCell>
                </TableHead>
                <TableBody>
                  {model.slots.map((slot) => (
                    <TableRow key={slot.name}>
                      <TableCell primary>{slot.name}</TableCell>
                      <TableCell>
                        {t(`models.affordance.${slotAffordance(slot, enumNames)}`)}
                      </TableCell>
                      <TableCell>
                        {slotDimension(slotAffordance(slot, enumNames)) ?? "—"}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            ) : null}
            {tab === "docs" ? (
              artifacts?.docs ? (
                <CodeBlock label={t("codeBlock.text")} className="max-h-96 rounded-lg border border-border bg-surface-subtle p-3 font-mono text-caption text-fg">
                  {artifacts.docs}
                </CodeBlock>
              ) : (
                <p className="text-body text-fg-muted">{t("models.noArtifact")}</p>
              )
            ) : null}
          </>
        )}
      </div>

      {artifacts?.generatorVersion ? (
        <p className="text-caption text-fg-muted">
          {t("models.generatedBy", { version: artifacts.generatorVersion })}
        </p>
      ) : null}
    </div>
  );
}
