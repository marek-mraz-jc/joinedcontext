/**
 * The form view of a space's type and the public form (T-3103, ADR-N-042 §3.2 and §3.5, API/01
 * §30 and §33).
 *
 * `FormView` asks for the fields of the type's LinkML class, as the saved view's settings choose,
 * order and relabel them, and creates one entity through the space surface with the person's own
 * session: the Policy decides the write. `FormSettingsEditor` is where the settings are made, and
 * `FormSharePanel` publishes the form: a public Endpoint whose Policy grants anyone `createEntity`
 * on the type alone. `PublicFormPage` is that link, `/f/{slug}`: it reads nothing but the
 * Endpoint's published schema and posts anonymously through the Endpoint.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent, JSX, RefObject } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { originTransport } from "@joinedcontext/sdk";
import { MAX_REASON, reasonProblem } from "../../components/entities/reason";
import { EntitySearchPicker } from "../../components/pickers/EntitySearchPicker";
import { Alert, Button, Checkbox, Field, Input, Select } from "../../components/ui";
import { endpointUrl } from "../../components/endpoints/links";
import type { LinkmlSlot } from "../models/linkml";
import {
  createdId,
  entityOf,
  fieldsOf,
  fieldsOfSchema,
  FORM_HEIGHT_MESSAGE,
  prefilled,
  problemsOf,
  trapName,
  visibleFields,
} from "./formView";
import type { Answers, Condition, FieldOption, FormField, FormSettings, Problem } from "./formView";

/** The answers a form holds and what changing one does. */
function useAnswers(initial: Answers) {
  const [answers, setAnswers] = useState<Answers>(initial);
  const set = (key: string, value: string) => setAnswers((before) => ({ ...before, [key]: value }));
  return { answers, set, reset: () => setAnswers(initial) };
}

/** The controls of the shown fields, each labelled, helped and saying what is wrong with it. */
export function FormFields({
  idPrefix,
  fields,
  answers,
  problems,
  onAnswer,
  space,
}: {
  /** A signed-in form's space: a relationship then picks an entity the person may read there. */
  space?: string;
  idPrefix: string;
  fields: FormField[];
  answers: Answers;
  problems: Record<string, Problem>;
  onAnswer: (key: string, value: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <>
      {fields.map((field) => {
        const id = `${idPrefix}-${field.attr}`;
        const problem = problems[field.attr];
        const errors = problem ? [t(`spaces.form.problem.${problem}`)] : undefined;
        if (field.kind === "boolean") {
          return (
            <Field key={field.attr} id={id} hideLabel help={field.help} errors={errors}>
              <Checkbox
                id={id}
                label={field.label}
                checked={answers[field.attr] === "true"}
                onChange={(event) => onAnswer(field.attr, event.target.checked ? "true" : "false")}
              />
            </Field>
          );
        }
        if (field.kind === "point") {
          return (
            <fieldset key={field.attr} className="flex flex-col gap-2" aria-describedby={problem ? `${id}-problem` : undefined}>
              <legend className="text-body font-semibold text-fg">
                {field.label}
                {field.required ? ` (${t("spaces.form.required")})` : ""}
              </legend>
              {field.help ? <p className="text-caption text-fg-muted">{field.help}</p> : null}
              <div className="flex flex-wrap gap-3">
                {(["lat", "lon"] as const).map((part) => (
                  <Field key={part} id={`${id}-${part}`} label={t(`spaces.form.${part}`)}>
                    <Input
                      id={`${id}-${part}`}
                      inputMode="decimal"
                      value={answers[`${field.attr}.${part}`] ?? ""}
                      onChange={(event) => onAnswer(`${field.attr}.${part}`, event.target.value)}
                    />
                  </Field>
                ))}
              </div>
              {problem ? (
                <p id={`${id}-problem`} className="text-caption text-danger">
                  {t(`spaces.form.problem.${problem}`)}
                </p>
              ) : null}
            </fieldset>
          );
        }
        return (
          <Field key={field.attr} id={id} label={field.label} required={field.required} help={field.help} errors={errors}>
            {field.kind === "relationship" && space && field.target ? (
              <EntitySearchPicker
                id={id}
                label={field.label}
                labelled
                space={space}
                type={field.target}
                value={answers[field.attr] ?? ""}
                onChange={(picked) => onAnswer(field.attr, picked)}
                required={field.required}
                invalid={Boolean(errors)}
              />
            ) : field.kind === "enum" ? (
              <Select id={id} value={answers[field.attr] ?? ""} onChange={(event) => onAnswer(field.attr, event.target.value)}>
                <option value="">{t("spaces.form.choose")}</option>
                {(field.options ?? []).map((option: FieldOption) => (
                  <option key={option.value} value={option.value}>
                    {option.title ?? option.value}
                  </option>
                ))}
              </Select>
            ) : (
              <Input
                id={id}
                type={field.kind === "date" ? "date" : field.kind === "datetime" ? "datetime-local" : "text"}
                inputMode={field.kind === "integer" ? "numeric" : field.kind === "number" ? "decimal" : undefined}
                placeholder={field.kind === "relationship" ? "urn:ngsi-ld:Type:id" : undefined}
                value={answers[field.attr] ?? ""}
                onChange={(event) => onAnswer(field.attr, event.target.value)}
              />
            )}
          </Field>
        );
      })}
    </>
  );
}

/** What one submission did. */
type Sent = { status: "idle" } | { status: "sending" } | { status: "created"; id: string } | { status: "refused"; reason: string };

function reasonOf(status: number, body: unknown): string {
  const problem = (body ?? {}) as { detail?: unknown; title?: unknown };
  return String(problem.detail ?? problem.title ?? `HTTP ${status}`);
}

/**
 * One form that creates an entity: the shown fields, their checks, and the send. `send` posts the
 * entity and answers the status and the body.
 */
function EntityFormBody({
  idPrefix,
  type,
  fields,
  conditions,
  initial,
  locale,
  send,
  trap,
  test = false,
  space,
  reasonDefault,
  forbiddenHint,
}: {
  idPrefix: string;
  type: string;
  fields: FormField[];
  conditions?: Condition[];
  initial: Answers;
  locale: string;
  /** Sends the entity and the person's reason; `id` is the one the server minted, when it names one. */
  send: (entity: Record<string, unknown>, reason: string) => Promise<{ status: number; body: unknown; id?: string }>;
  /** A public form's trap field: hidden from people, sent as an attribute no Policy grants. */
  trap?: string;
  /** A test run (EP-102): `send` writes nothing, and its answer says what a real one would do. */
  test?: boolean;
  /** The signed-in form's space; a public form has none and reads nothing. */
  space?: string;
  /**
   * The signed-in form asks why the person writes (DM-113), starting from this sentence; a public
   * form is anonymous and asks nothing.
   */
  reasonDefault?: string;
  /** What the person can do about a 403: only the signed-in form has a project owner to ask. */
  forbiddenHint?: string;
}): JSX.Element {
  const { t } = useTranslation();
  const { answers, set, reset } = useAnswers(initial);
  const [trapped, setTrapped] = useState("");
  const [tried, setTried] = useState(false);
  const [sent, setSent] = useState<Sent>({ status: "idle" });
  const [reason, setReason] = useState(reasonDefault ?? "");
  const shown = visibleFields(fields, conditions, answers);
  const problems = problemsOf(shown, answers);
  const reasonError = reasonDefault === undefined ? null : reasonProblem(reason);
  const count = Object.keys(problems).length + (reasonError ? 1 : 0);

  // A long form puts the refusal under the fold: the person pressed Create and saw nothing.
  const refusal = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (sent.status === "refused") refusal.current?.scrollIntoView?.({ block: "center" });
  }, [sent]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setTried(true);
    if (count > 0) return;
    setSent({ status: "sending" });
    const entity = entityOf(type, shown, answers, locale);
    if (trap && trapped !== "") entity[trap] = { type: "Property", value: trapped };
    try {
      const answer = await send(entity, reason.trim());
      if (answer.status >= 200 && answer.status < 300) {
        setSent({ status: "created", id: answer.id ?? String(entity.id) });
        setTried(false);
        reset();
      } else {
        const said = reasonOf(answer.status, answer.body);
        setSent({ status: "refused", reason: answer.status === 403 && forbiddenHint ? `${said}. ${forbiddenHint}` : said });
      }
    } catch (error) {
      setSent({ status: "refused", reason: error instanceof Error ? error.message : String(error) });
    }
  };

  return (
    <form className="flex max-w-xl flex-col gap-3" noValidate onSubmit={(event) => void submit(event)}>
      <FormFields idPrefix={idPrefix} fields={shown} answers={answers} problems={tried ? problems : {}} onAnswer={set} space={space} />
      {reasonDefault !== undefined ? (
        <Field
          id={`${idPrefix}-reason`}
          label={t("spaces.form.reason")}
          required
          help={t("spaces.form.hint.reason")}
          errors={tried && reasonError ? [t(`spaces.form.reasonError.${reasonError}`, { max: MAX_REASON })] : undefined}
        >
          <Input
            id={`${idPrefix}-reason`}
            value={reason}
            maxLength={MAX_REASON}
            autoComplete="off"
            onChange={(event) => setReason(event.target.value)}
          />
        </Field>
      ) : null}
      {trap ? (
        <div aria-hidden="true" className="absolute -left-[10000px] h-px w-px overflow-hidden">
          <label htmlFor={`${idPrefix}-${trap}`}>{t("spaces.form.trap")}</label>
          <Input id={`${idPrefix}-${trap}`} name={trap} tabIndex={-1} autoComplete="off" value={trapped} onChange={(event) => setTrapped(event.target.value)} />
        </div>
      ) : null}
      {tried && count > 0 ? (
        <Alert role="alert" tone="danger">
          {t("spaces.form.fix", { count })}
        </Alert>
      ) : null}
      {sent.status === "refused" ? (
        <div ref={refusal}>
          <Alert role="alert" tone="danger">
            {t(test ? "spaces.form.testRefused" : "spaces.form.refused", { reason: sent.reason })}
          </Alert>
        </div>
      ) : null}
      {sent.status === "created" ? (
        <p role="status" className="text-body text-fg [overflow-wrap:anywhere]">
          {test ? t("spaces.form.testPassed") : t("spaces.form.created", { id: sent.id })}
        </p>
      ) : null}
      <Button type="submit" className="w-fit" disabled={sent.status === "sending"} disabledReason={t("app.loading")}>
        {t(test ? "spaces.form.testSubmit" : "spaces.form.submit")}
      </Button>
    </form>
  );
}

/** The signed-in form of a type: it creates through the space surface with this session. */
export function FormView({
  space,
  type,
  slots,
  enums,
  settings,
  search = new URLSearchParams(window.location.search),
  onCreated,
}: {
  space: string;
  type: string;
  slots: LinkmlSlot[];
  enums: Record<string, FieldOption[]>;
  settings?: FormSettings;
  search?: URLSearchParams;
  /** Called after the space took the new entity, for a host that reads its rows again (T-4828). */
  onCreated?: () => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const fields = useMemo(() => fieldsOf(slots, enums, settings, i18n.language), [slots, enums, settings, i18n.language]);
  const initial = useMemo(() => prefilled(fields, search, settings), [fields, search, settings]);
  if (fields.length === 0) {
    return <p className="text-body text-fg-muted">{t("spaces.form.noFields", { type })}</p>;
  }
  return (
    <EntityFormBody
      // A view or a type chosen anew is a fresh form: nothing typed carries over.
      key={`${type}-${JSON.stringify(settings ?? {})}`}
      idPrefix="space-form"
      space={space}
      type={type}
      fields={fields}
      conditions={settings?.conditions}
      initial={initial}
      locale={i18n.language}
      reasonDefault={t("spaces.form.reasonDefault")}
      forbiddenHint={t("spaces.form.forbidden", { type })}
      send={async (entity, reason) => {
        const answer = await originTransport()({ method: "POST", path: `/cs/${encodeURIComponent(space)}/ngsi-ld/v1/entities`, body: entity, reason });
        if (answer.status >= 200 && answer.status < 300) onCreated?.();
        return answer;
      }}
    />
  );
}

/** An anonymous read of the gateway: no cookie or other credential goes with it (EP-102). */
async function anonymousJson(url: string): Promise<unknown> {
  const response = await globalThis.fetch(new Request(url, { headers: { Accept: "application/json" }, credentials: "omit" }));
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.json();
}

/** The answer a send reads: status, body, and the id the gateway minted. */
async function answerOf(response: Response): Promise<{ status: number; body: unknown; id?: string }> {
  const text = await response.text();
  let body: unknown = undefined;
  try {
    body = text ? JSON.parse(text) : undefined;
  } catch {
    body = { title: text };
  }
  return { status: response.status, body, id: createdId(response.headers.get("Location")) };
}

/** Tells the page that frames this one how tall it is, the only message it sends (API/01 §33). */
function useHeightToParent(): RefObject<HTMLDivElement | null> {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const element = ref.current;
    if (!element || window.parent === window || typeof ResizeObserver === "undefined") return;
    const post = () =>
      // The height is no secret, and the framing site's origin is not known to the page.
      window.parent.postMessage({ type: FORM_HEIGHT_MESSAGE, height: Math.ceil(document.documentElement.scrollHeight) }, "*");
    const observer = new ResizeObserver(post);
    observer.observe(element);
    post();
    return () => observer.disconnect();
  }, []);
  return ref;
}

interface SchemaIndex {
  models?: { version?: number }[];
}

/**
 * The type the Endpoint publishes: a public form's Endpoint names exactly one (API/01 §19), the one
 * definition with `id` and `type` properties; a nested object's definition is none. Two or none
 * leave the form unpublished rather than guess.
 */
export function publishedType(defs: Record<string, unknown>): string | undefined {
  const entities = Object.entries(defs).filter(([, definition]) => {
    const properties = (definition as { properties?: unknown } | null)?.properties;
    return typeof properties === "object" && properties !== null && "id" in properties && "type" in properties;
  });
  return entities.length === 1 ? entities[0][0] : undefined;
}

/** The page at `/f/{slug}`: a public form, built from what the Endpoint publishes and sent through it. */
export function PublicFormPage({
  slug,
  search = new URLSearchParams(window.location.search),
  headingLevel = 1,
}: {
  slug: string;
  search?: URLSearchParams;
  /** 1 on its own page; the share panel's preview shows it under its own headings. */
  headingLevel?: 1 | 4;
}): JSX.Element {
  const Heading = headingLevel === 1 ? "h1" : "h4";
  const { t, i18n } = useTranslation();
  const frameRef = useHeightToParent();
  const test = search.get("test") === "1";
  const base = endpointUrl(slug, "/schema");
  const schema = useQuery({
    queryKey: ["public-form", slug],
    retry: false,
    queryFn: async () => {
      const index = (await anonymousJson(`${base}/index.json`)) as SchemaIndex;
      const version = index.models?.[0]?.version ?? 1;
      return (await anonymousJson(`${base}/v${version}/json-schema`)) as {
        definitions?: Record<string, unknown>;
        $defs?: Record<string, unknown>;
      };
    },
  });
  const defs = useMemo(() => ({ ...schema.data?.$defs, ...schema.data?.definitions }), [schema.data]);
  const type = publishedType(defs);
  const fields = useMemo(() => (type ? fieldsOfSchema(defs[type], defs) : []), [defs, type]);
  const initial = useMemo(() => prefilled(fields, search, undefined), [fields, search]);

  if (schema.isPending || schema.isError || !type || fields.length === 0) {
    return (
      <div ref={frameRef} className="flex flex-col gap-3" data-testid="public-form">
        <Heading className="text-title font-semibold text-fg">{type ?? t("spaces.form.publicTitle")}</Heading>
        <p className="text-body text-fg-muted" role="status">
          {schema.isPending ? t("app.loading") : t("spaces.form.notPublished")}
        </p>
      </div>
    );
  }
  return (
    <div ref={frameRef} className="flex flex-col gap-3" data-testid="public-form">
      <Heading className="text-title font-semibold text-fg">{type}</Heading>
      <p className="text-body text-fg-muted">{t("spaces.form.publicLead")}</p>
      {test ? <Alert tone="warning">{t("spaces.form.testMode")}</Alert> : <Alert tone="info">{t("spaces.form.publicData")}</Alert>}
      <EntityFormBody
        idPrefix="public-form"
        type={type}
        fields={fields}
        initial={initial}
        locale={i18n.language}
        trap={trapName(fields)}
        test={test}
        send={async (entity) => {
          if (test) {
            // A test writes nothing: the fields were checked against the published schema above,
            // and the gateway's PDP, the one that decides writes, says whether it would take one.
            const response = await globalThis.fetch(
              new Request(endpointUrl(slug, "/access/check"), {
                method: "POST",
                headers: { "Content-Type": "application/json", Accept: "application/json" },
                body: JSON.stringify({ action: { name: "createEntity" }, resource: { type } }),
                credentials: "omit",
              }),
            );
            const answer = await answerOf(response);
            const decision = (answer.body as { decision?: unknown } | undefined)?.decision;
            if (answer.status === 200 && decision !== true) return { status: 403, body: { title: t("spaces.form.testNotAllowed") } };
            return answer;
          }
          // Anonymous: no Portal session goes with it; the gateway decides and rate-limits it.
          return answerOf(
            await globalThis.fetch(
              new Request(endpointUrl(slug, "/ngsi-ld/v1/entities"), {
                method: "POST",
                headers: { "Content-Type": "application/json", Accept: "application/json" },
                body: JSON.stringify(entity),
                credentials: "omit",
              }),
            ),
          );
        }}
      />
    </div>
  );
}
