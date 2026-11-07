import { useMemo, useRef, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError } from "../../api/client";
import type { ProblemDetails } from "../../api/client";
import { SCHEMA_FORMALISMS } from "../../schemas/kinds";
import type { SchemaFormalism } from "../../schemas/kinds";
import { CodeBlock,
  Alert,
  Button,
  Checkbox,
  EmptyState,
  Field,
  Input,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableRowHeaderCell,
} from "../../components/ui";
import { FormHeading } from "../../components/forms/FormRoute";

/**
 * What an Endpoint publishes of its model, and which attributes it holds back (EP-46, EP-61).
 *
 * The attribute names are read from the endpoint's own schema surface rather than from the
 * DataModel manifest, because the surface is already projected to the policy: what it lists
 * is what this endpoint may publish at all, so ticking a box here can only narrow further
 * (R9, EP-07). An attribute the steward hides is struck from the published column and never
 * reaches the preview, which is the same rule the gateway applies to the served document.
 */

/** One class of the published JSON Schema and the attributes it still carries. */
export interface PublishedType {
  name: string;
  attributes: string[];
}

interface SchemaIndex {
  models?: { name?: string; version?: number }[];
}

/**
 * Reads the types of the projected document the endpoint serves: draft-07 keeps them under
 * `definitions`, a later draft under `$defs`, and the gateway has served both.
 */
export function publishedTypes(document: unknown): PublishedType[] {
  const doc = document as { $defs?: Record<string, unknown>; definitions?: Record<string, unknown> } | null;
  return Object.entries({ ...doc?.$defs, ...doc?.definitions })
    .map(([name, definition]) => ({
      name,
      attributes: Object.keys(
        (definition as { properties?: Record<string, unknown> })?.properties ?? {},
      ).sort(),
    }))
    .filter((type) => type.attributes.length > 0);
}

/** A `Request` rather than a bare URL, so the caller reads like every other call the app makes. */
async function get(url: string, accept: string): Promise<Response> {
  const answer = await fetch(new Request(url, { headers: { Accept: accept } }));
  if (!answer.ok) {
    // The status alone was the whole message, so every caller that shows a failed schema fetch
    // showed a person the bare number "502" and the Problem Details sentence the surface had
    // just sent was dropped on the floor. An unreadable body leaves the status, as before.
    // The problem and the edge's request id go along, so a failure state reads the sentence, the
    // hint and the reference as it does for every other call (PageFailed, T-6313).
    let problem: ProblemDetails | undefined;
    try {
      const body: unknown = await answer.clone().json();
      problem = body && typeof body === "object" && !Array.isArray(body) ? (body as ProblemDetails) : undefined;
    } catch {
      problem = undefined;
    }
    const detail = typeof problem?.detail === "string" ? problem.detail : undefined;
    throw new ApiError(answer.status, detail ?? String(answer.status), problem, answer.headers.get("x-request-id") ?? undefined);
  }
  return answer;
}

export async function fetchJson(url: string): Promise<unknown> {
  return (await get(url, "application/json")).json();
}

async function fetchText(url: string): Promise<string> {
  return (await get(url, "*/*")).text();
}

export interface SchemaProjectionPanelProps {
  /** The endpoint's slug; the schema surface lives under it. */
  slug: string;
  /** Attribute names this endpoint hides, `spec.projection.hiddenAttributes`. */
  hidden: string[];
  onHiddenChange: (hidden: string[]) => void;
  /**
   * Why this person may not change the projection, when they may not (UI-44).
   *
   * Without it a viewer ticked every box, typed new names and learned that none of it could be
   * proposed only when the Propose button refused them — the sequence UI-44 exists to prevent.
   */
  disabledReason?: string;
}

/**
 * What is wrong with a typed attribute name, or nothing.
 *
 * A name the published schema does not carry is NOT wrong: an attribute can be hidden before the
 * model that introduces it is compiled, and `endpoint_projection_ui.test.tsx` holds that case.
 * What such a name is, is easy to mistake for a hidden attribute when it is a typo, so it is
 * accepted and then said aloud under the field rather than refused here (T-1767).
 */
function faultOf(name: string, hidden: string[]): string | null {
  if (name === "") return "endpoints.projection.addEmpty";
  if (hidden.includes(name)) return "endpoints.projection.addAlready";
  // A name with a space in it is not an attribute name under any model, published or not.
  if (!/^[A-Za-z_][A-Za-z0-9_:.-]*$/.test(name)) return "endpoints.projection.addShape";
  return null;
}

export function SchemaProjectionPanel({
  slug,
  hidden,
  onHiddenChange,
  disabledReason,
}: SchemaProjectionPanelProps): JSX.Element {
  const { t } = useTranslation();
  const [formalism, setFormalism] = useState<SchemaFormalism>("json-schema");
  const [typed, setTyped] = useState("");
  const [fault, setFault] = useState<string | null>(null);
  const typedRef = useRef<HTMLInputElement | null>(null);
  const refused = Boolean(disabledReason);
  const base = `${window.location.origin}/api/endpoint/${slug}/schema`;

  const index = useQuery({
    queryKey: ["endpoint-schema-index", slug],
    retry: false,
    queryFn: () => fetchJson(`${base}/index.json`) as Promise<SchemaIndex>,
  });

  // The major of the first model is the version segment every artifact of it lives under.
  const version = index.data?.models?.[0]?.version ?? 1;
  const artifactUrl = `${base}/v${version}/${formalism}`;

  const schema = useQuery({
    queryKey: ["endpoint-schema-artifact", slug, version, formalism],
    enabled: index.isSuccess,
    retry: false,
    queryFn: () =>
      formalism === "json-schema"
        ? (fetchJson(artifactUrl) as Promise<unknown>)
        : fetchText(artifactUrl),
  });

  const types = useMemo(
    () => (formalism === "json-schema" ? publishedTypes(schema.data) : []),
    [formalism, schema.data],
  );

  /** Every attribute name this endpoint publishes, when the schema says. */
  const published = useMemo(() => types.flatMap((type) => type.attributes), [types]);
  // Hidden names that match nothing published today. Legitimate before a model is compiled, and
  // a typo the rest of the time; either way the person is told rather than left with a chip that
  // looks like a withheld attribute.
  const hidesNothing = useMemo(
    () => (published.length === 0 ? [] : hidden.filter((name) => !published.includes(name))),
    [hidden, published],
  );

  const toggle = (attribute: string) => {
    onHiddenChange(
      hidden.includes(attribute)
        ? hidden.filter((name) => name !== attribute)
        : [...hidden, attribute],
    );
  };

  return (
    <section aria-labelledby="projection-heading" className="space-y-3">
      <FormHeading id="projection-heading" className="text-base font-semibold">
        {t("endpoints.projection.title")}
      </FormHeading>
      <p id="projection-hint" className="text-sm text-fg-muted">{t("endpoints.projection.hint")}</p>

      <Field id="projection-formalism" label={t("endpoints.projection.formalism")} description={t("endpoints.projection.formalismHelp")}>
        <Select
          id="projection-formalism"
          value={formalism}
          onChange={(event) => setFormalism(event.target.value as SchemaFormalism)}
        >
          {SCHEMA_FORMALISMS.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </Select>
      </Field>
      <p className="break-all font-mono text-caption text-fg-muted">{artifactUrl}</p>

      {/* The artifact query is `enabled: index.isSuccess`, and a disabled query stays
          `status: "pending"` for ever: reading `schema.isPending` on its own showed "reading the
          schema" beside "no schema yet" until the panel was closed (T-1767). */}
      {index.isPending || (index.isSuccess && schema.isPending) ? (
        <p role="status">{t("endpoints.projection.loading")}</p>
      ) : null}

      {index.isError ? (
        <Alert
          tone="danger"
          actions={
            <Button variant="secondary" size="xs" onClick={() => void index.refetch()}>
              {t("form.listRetry")}
            </Button>
          }
        >
          {t("endpoints.projection.unavailable")}
        </Alert>
      ) : null}

      {index.isSuccess && schema.isError ? (
        <Alert
          tone="warning"
          actions={
            <Button variant="secondary" size="xs" onClick={() => void schema.refetch()}>
              {t("form.listRetry")}
            </Button>
          }
        >
          {t("endpoints.projection.notCompiled")}
        </Alert>
      ) : null}

      {/* The artifact was read and names nothing: not a failure, and not silence either. */}
      {formalism === "json-schema" && schema.isSuccess && types.length === 0 ? (
        <EmptyState
          bare
          title={t("endpoints.projection.noTypes")}
          description={t("endpoints.projection.noTypesHint")}
          icon="models"
        />
      ) : null}

      {formalism !== "json-schema" && schema.isSuccess ? (
        <CodeBlock label={t("codeBlock.schema")} className="max-h-64 rounded border border-border bg-surface-subtle p-3 text-xs">
          {String(schema.data)}
        </CodeBlock>
      ) : null}

      {types.map((type) => {
        // The preview is built by subtraction, so a hidden attribute cannot appear in it
        // even if the published document still carries it.
        const effective = type.attributes.filter((name) => !hidden.includes(name));
        return (
          // The class name is a heading a person reads, and the table's own name for a screen
          // reader: the shared Table hides its caption, so it is written once above and passed in.
          <section key={type.name} aria-labelledby={`projection-${type.name}`} className="space-y-1">
            <FormHeading id={`projection-${type.name}`} className="font-medium text-fg">
              {type.name}
            </FormHeading>
            <Table caption={type.name}>
              <TableHead>
                <TableHeaderCell>{t("endpoints.projection.attribute")}</TableHeaderCell>
                <TableHeaderCell>{t("endpoints.projection.hide")}</TableHeaderCell>
              </TableHead>
              <TableBody>
                {type.attributes.map((attribute) => (
                  <TableRow key={attribute}>
                    <TableRowHeaderCell className="font-mono font-normal">
                      {attribute}
                    </TableRowHeaderCell>
                    <TableCell>
                      <Checkbox
                        // The column header says "Hide"; the box itself still has to name the
                        // attribute it hides, or a screen reader reads a row of bare checkboxes.
                        label={
                          <span className="sr-only">{`${t("endpoints.projection.hide")} ${attribute}`}</span>
                        }
                        // a refused box is described by its reason instead
                        aria-describedby={refused ? undefined : "projection-hint"}
                        checked={hidden.includes(attribute)}
                        disabled={refused}
                        disabledReason={disabledReason}
                        onChange={() => toggle(attribute)}
                      />
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            {/* The summary of the table above, not a row of it: a `tfoot` cell spanning both
                columns was read as data by a screen reader walking the rows. */}
            <p className="text-sm">
              <span className="font-medium">{t("endpoints.projection.effective")}: </span>
              {effective.length > 0 ? (
                <span className="font-mono">{effective.join(", ")}</span>
              ) : (
                <span className="text-fg-muted">{t("endpoints.projection.none")}</span>
              )}
            </p>
          </section>
        );
      })}

      {/* A text field and the button that takes it: a form, so Enter works and the button is
          the form's submit rather than a click handler beside an input. */}
      <form
        className="flex items-end gap-2"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          if (refused) return;
          const name = typed.trim();
          const wrong = faultOf(name, hidden);
          if (wrong) {
            setFault(wrong);
            typedRef.current?.focus();
            return;
          }
          onHiddenChange([...hidden, name]);
          setTyped("");
        }}
      >
        <Field
          id="projection-typed"
          label={t("endpoints.projection.addHidden")}
          className="flex-1"
          help={
            hidesNothing.length > 0
              ? t("endpoints.projection.addUnknown", { names: hidesNothing.join(", ") })
              : published.length > 0
                ? t("endpoints.projection.addHint")
                : undefined
          }
          errors={fault ? [t(fault)] : undefined}
        >
          <Input
            id="projection-typed"
            ref={typedRef}
            value={typed}
            onChange={(event) => {
              setTyped(event.target.value);
              setFault(null);
            }}
          />
        </Field>
        <Button type="submit" disabled={refused} disabledReason={disabledReason}>
          {t("endpoints.projection.add")}
        </Button>
      </form>

      {hidden.length > 0 ? (
        <ul className="flex flex-wrap gap-1">
          {hidden.map((attribute) => (
            <li key={attribute}>
              <Button
                variant="secondary"
                size="xs"
                className="font-mono"
                onClick={() => toggle(attribute)}
              >
                {attribute}
                <span aria-hidden="true">×</span>
                {/* This chip takes the attribute back OUT of the hidden list, and announced
                    "Hide": someone unhiding a field heard that they were hiding it, and the
                    endpoint then published a field they believed they had withheld. */}
                <span className="sr-only">{t("endpoints.projection.unhide")}</span>
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}
