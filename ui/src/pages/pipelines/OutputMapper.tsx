/**
 * The output node's mapper (T-3224): pick the entity type of the target space, see its attributes
 * as the space's data model states them (API/01 §36), and map each one to a field of the sample,
 * an expression, a fixed value or a point. It writes the pipeline's Bloblang compute step
 * (`outputMapper.ts`), so the pipeline test and the runner judge exactly what is saved.
 */
import { useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError } from "../../api/client";
import type { components } from "../../api/schema";
import { RecordLink } from "../../components/RecordLink";
import { TypePicker } from "../../components/pickers/TypePicker";
import { CodeBlock, Alert, Badge, Button, Field, Input, Select } from "../../components/ui";
import type { Expiry, Lifetime, LifetimeMode } from "./lifetime";
import { autoMatch, generate, idFieldOf, migrated, preview, problemsOf, readBack } from "./outputMapper";
import type { Attribute, MapperProblem, MapperState, Source } from "./outputMapper";

export interface OutputMapperProps {
  project: string;
  /** The target endpoint's space, whose model the type comes from. */
  space: string;
  orgDomain: string;
  /** The records the source reads, as the sample step shows them. */
  records: Record<string, unknown>[];
  /** The compute step as it stands. */
  bloblang: string;
  onBloblang: (bloblang: string) => void;
  /** The pipeline's stale-entity sweep (PL-64), offered beside the entities' own `expiresAt`. */
  expiry?: Expiry;
  /** The pipeline's `spec.lifetime` (PL-80): the entity's end of the type mapped here is edited. */
  lifetime?: Lifetime[];
  /**
   * Sets the sweep, the lifetime and, when it changes too, the step: one change, so none
   * overwrites another. Absent, the mapper offers no end time at all, since nothing would keep it.
   */
  onEnds?: (ends: Ends) => void;
}

/** What `onEnds` sets together (T-4839). */
export interface Ends {
  expiry: Expiry | undefined;
  lifetime: Lifetime[] | undefined;
  bloblang?: string;
}

/** `lifetime` with the entity end of `type` set, or removed when `entity` is undefined. */
export function withEntityEnd(lifetime: Lifetime[] | undefined, type: string, entity: LifetimeMode | undefined): Lifetime[] | undefined {
  const others = (lifetime ?? []).filter((entry) => entry.type !== type);
  const own = (lifetime ?? []).find((entry) => entry.type === type);
  const rest = own?.attributes?.length ? { type, attributes: own.attributes } : undefined;
  const next = entity ? [...others, { ...(rest ?? { type }), entity }] : rest ? [...others, rest] : others;
  return next.length > 0 ? next : undefined;
}

type Choice = "" | "expression" | "value" | "point" | `field:${string}`;

const choiceOf = (source: Source | undefined): Choice => {
  if (!source) return "";
  if ("field" in source) return `field:${source.field}`;
  if ("expression" in source) return "expression";
  if ("value" in source) return "value";
  return "point";
};

type ClassAttributes = components["schemas"]["ClassAttributes"];

export async function attributesOf(project: string, space: string, type: string): Promise<ClassAttributes> {
  const { data, error, response } = await api.GET("/api/v1/projects/{project}/spaces/{space}/types/{type}/attributes", {
    params: { path: { project, space, type } },
  });
  if (data) return data;
  const detail = (error as { detail?: unknown } | undefined)?.detail;
  throw new ApiError(response.status, typeof detail === "string" ? detail : response.statusText);
}

export function OutputMapper({ project, space, orgDomain, records, bloblang, onBloblang, expiry, lifetime, onEnds }: OutputMapperProps): JSX.Element {
  const { t } = useTranslation();
  const fields = useMemo(() => [...new Set(records.flatMap((record) => Object.keys(record)))], [records]);
  // The type the step writes: from its own map once the attributes are known, else the one picked.
  const [picked, setPicked] = useState<string | undefined>(() => {
    const header = bloblang.startsWith("# jc-mapper ") ? bloblang.slice(12).split("\n", 1)[0] : "";
    try {
      return (JSON.parse(header) as { type?: string }).type;
    } catch {
      return undefined;
    }
  });
  const [kept, setKept] = useState<string | null>(null);

  const typed = useQuery({
    queryKey: ["type-attributes", project, space, picked],
    enabled: Boolean(picked),
    queryFn: () => attributesOf(project, space, picked ?? ""),
    retry: false,
  });
  const attributes = useMemo(() => typed.data?.attributes ?? [], [typed.data]);
  const state = picked && typed.data ? readBack(bloblang, attributes, space) : undefined;
  const handWritten = bloblang.trim() !== "" && typed.data !== undefined && state === undefined && !bloblang.startsWith("# jc-mapper ");

  const write = (next: MapperState) => onBloblang(generate(next, attributes, space));
  const current: MapperState | undefined =
    state ?? (picked && typed.data && !handWritten ? { type: picked, idField: idFieldOf(fields), attributes: autoMatch(attributes, fields) } : undefined);
  // The type's own entry of `spec.lifetime`; one the choice below cannot show (a fixed date, a
  // cap, chosen attributes) stays as the manifest holds it.
  const own = current ? lifetime?.find((entry) => entry.type === current.type) : undefined;
  const entityEnd = own?.entity;
  const problems = current ? problemsOf(current, attributes, records, entityEnd) : [];
  // A T-4822 step wrote its own `expiresAt`: it moves into `spec.lifetime` once, on open, and is
  // saved with the next proposal; a record field no date-time attribute holds waits for a choice.
  // A manifest that already holds an entry for the type keeps it, and the step's lines go.
  const legacy = state?.expires;
  const moved = legacy && state ? migrated(legacy, state, attributes) : undefined;
  const movedTo = state?.expires && onEnds ? (own ? lifetime : moved ? withEntityEnd(lifetime, state.type, moved) : undefined) : undefined;
  // Which type's step was moved, kept past the move (the step no longer says so) until the person
  // changes the end themselves; set while rendering, the way React keeps what an earlier render saw.
  const [movedType, setMovedType] = useState<string | undefined>(undefined);
  if (movedTo && state && movedType !== state.type) setMovedType(state.type);
  const notice = legacy && !movedTo && onEnds ? "migrateField" : movedType !== undefined && movedType === current?.type ? "migrated" : undefined;
  useEffect(() => {
    if (!state?.expires || !onEnds || !movedTo) return;
    onEnds({ expiry, lifetime: movedTo, bloblang: generate({ ...state, expires: undefined }, attributes, space) });
  }, [state, movedTo, onEnds, expiry, attributes, space]);

  // A type picked for an empty step, or for one this mapper wrote for another type, writes the
  // step at once with the fields matched by name. A step written by hand is never replaced here.
  const fresh = bloblang.trim() === "" || bloblang.startsWith("# jc-mapper ");
  const due = state === undefined && fresh && current && typed.data ? generate(current, typed.data.attributes, space) : undefined;
  useEffect(() => {
    // Once written, the step reads back as `state` and nothing is due any more.
    if (due !== undefined && due !== bloblang) onBloblang(due);
  }, [due, bloblang, onBloblang]);
  const problemOf = (name: string) => problems.find((problem) => problem.attribute === name);

  const say = (problem: MapperProblem, name: string): string => {
    switch (problem.kind) {
      case "required":
        return t("pipelines.mapper.problem.required", { name });
      case "notInSample":
        return t("pipelines.mapper.problem.notInSample", { field: problem.field });
      case "notNumber":
        return t("pipelines.mapper.problem.notNumber", { field: problem.field, value: problem.value, name });
      case "notBoolean":
        return t("pipelines.mapper.problem.notBoolean", { field: problem.field, value: problem.value, name });
      case "notValue":
        return t("pipelines.mapper.problem.notValue", { field: problem.field, value: problem.value, name, values: problem.values.join(", ") });
      case "noId":
        return t("pipelines.mapper.problem.noId");
      case "notTime":
        return t("pipelines.mapper.problem.notTime", { field: problem.field, value: problem.value });
    }
  };

  const setSource = (name: string, source: Source | undefined) => {
    if (!current) return;
    const next = { ...current.attributes };
    if (source) next[name] = source;
    else delete next[name];
    write({ ...current, attributes: next });
  };

  const choose = (attribute: Attribute, choice: Choice) => {
    if (choice === "") setSource(attribute.name, undefined);
    else if (choice === "expression") setSource(attribute.name, { expression: "this" });
    else if (choice === "value") setSource(attribute.name, { value: attribute.values?.[0] ?? "" });
    else if (choice === "point") setSource(attribute.name, { longitude: fields[0] ?? "", latitude: fields[1] ?? fields[0] ?? "" });
    else setSource(attribute.name, { field: choice.slice(6) });
  };

  const noId = problems.find((problem) => problem.kind === "noId");
  const expiresProblem = problemOf("expiresAt");

  return (
    <div className="flex flex-col gap-3" data-testid="output-mapper">
      <Field id="mapper-type" label={t("pipelines.mapper.type")} description={t("pipelines.mapper.typeHint")} required>
        <TypePicker
          id="mapper-type"
          label={t("pipelines.mapper.type")}
          labelled
          project={project}
          space={space}
          value={picked ? [picked] : []}
          onChange={(types) => setPicked(types[0])}
        />
      </Field>

      {typed.isFetching ? (
        <p role="status" className="text-caption text-fg-subtle">
          {t("pipelines.mapper.loading")}
        </p>
      ) : typed.isError ? (
        <Alert role="alert" tone="danger">
          {t("pipelines.mapper.failed", { reason: typed.error instanceof Error ? typed.error.message : String(typed.error) })}
        </Alert>
      ) : null}

      {handWritten ? (
        <Alert role="status" tone="info">
          <span>{t("pipelines.mapper.handWritten")}</span>{" "}
          <Button
            size="sm"
            variant="secondary"
            onClick={() => {
              setKept(bloblang);
              if (picked) write({ type: picked, idField: idFieldOf(fields), attributes: autoMatch(attributes, fields) });
            }}
          >
            {t("pipelines.mapper.replace")}
          </Button>
        </Alert>
      ) : null}
      {kept !== null && state ? (
        <p className="text-caption text-fg-muted">
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              onBloblang(kept);
              setKept(null);
            }}
          >
            {t("pipelines.mapper.restore")}
          </Button>
        </p>
      ) : null}

      {current && typed.data ? (
        <>
          {typed.data.description ? <p className="text-caption text-fg-muted">{typed.data.description}</p> : null}
          {records.length === 0 ? <p className="text-caption text-fg-subtle">{t("pipelines.mapper.noSample")}</p> : null}
          <Field
            id="mapper-id"
            label={t("pipelines.mapper.id")}
            description={t("pipelines.mapper.idHint", { type: current.type, space })}
            errors={noId ? [say(noId, "")] : undefined}
            required
          >
            <Select
              id="mapper-id"
              value={current.idField ?? ""}
              onChange={(event) => write({ ...current, idField: event.target.value || undefined })}
            >
              <option value="">{t("pipelines.mapper.notMapped")}</option>
              {fields.map((field) => (
                <option key={field} value={field}>
                  {field}
                </option>
              ))}
            </Select>
          </Field>

          <ul className="flex flex-col gap-2" aria-label={t("pipelines.mapper.attributes", { type: current.type })}>
            {attributes.map((attribute) => {
              const source = current.attributes[attribute.name];
              const problem = problemOf(attribute.name);
              const id = `mapper-${attribute.name}`;
              return (
                <li key={attribute.name} className="rounded border border-border p-2" data-testid={`mapper-attribute-${attribute.name}`}>
                  <div className="flex flex-wrap items-center gap-1.5 text-caption">
                    <span className="font-mono font-medium text-fg">{attribute.name}</span>
                    <Badge mono>{attribute.values ? "enum" : (attribute.valueType ?? "any")}</Badge>
                    {attribute.kind !== "Property" ? <Badge>{attribute.kind}</Badge> : null}
                    {attribute.required ? <Badge tone="warning">{t("pipelines.mapper.required")}</Badge> : null}
                    {attribute.unit?.code ? <Badge mono>{t("pipelines.mapper.unit", { code: attribute.unit.code })}</Badge> : null}
                    {attribute.relationship ? <Badge>{t("pipelines.mapper.target", { target: attribute.relationship.target })}</Badge> : null}
                  </div>
                  {attribute.description ? <p className="text-caption text-fg-muted">{attribute.description}</p> : null}
                  <Field id={id} label={t("pipelines.mapper.sourceOf", { name: attribute.name })} errors={problem ? [say(problem, attribute.name)] : undefined}>
                    <Select id={id} value={choiceOf(source)} onChange={(event) => choose(attribute, event.target.value as Choice)}>
                      <option value="">{t("pipelines.mapper.notMapped")}</option>
                      {fields.length > 0 ? (
                        <optgroup label={t("pipelines.mapper.fields")}>
                          {fields.map((field) => (
                            <option key={field} value={`field:${field}`}>
                              {field}
                            </option>
                          ))}
                        </optgroup>
                      ) : null}
                      {source && "field" in source && !fields.includes(source.field) ? (
                        <option value={`field:${source.field}`}>{source.field}</option>
                      ) : null}
                      <option value="expression">{t("pipelines.mapper.expression")}</option>
                      <option value="value">{t("pipelines.mapper.fixedValue")}</option>
                      {attribute.kind === "GeoProperty" ? <option value="point">{t("pipelines.mapper.point")}</option> : null}
                    </Select>
                  </Field>
                  {source && "expression" in source ? (
                    <Field id={`${id}-expression`} label={t("pipelines.mapper.expressionOf", { name: attribute.name })}>
                      <Input
                        id={`${id}-expression`}
                        className="font-mono"
                        spellCheck={false}
                        value={source.expression}
                        onChange={(event) => setSource(attribute.name, { expression: event.target.value })}
                      />
                    </Field>
                  ) : null}
                  {source && "value" in source ? (
                    <Field id={`${id}-value`} label={t("pipelines.mapper.valueOf", { name: attribute.name })}>
                      {attribute.values ? (
                        <Select id={`${id}-value`} value={String(source.value)} onChange={(event) => setSource(attribute.name, { value: event.target.value })}>
                          {attribute.values.map((value) => (
                            <option key={value} value={value}>
                              {value}
                            </option>
                          ))}
                        </Select>
                      ) : (
                        <Input
                          id={`${id}-value`}
                          type={attribute.valueType === "number" || attribute.valueType === "integer" ? "number" : "text"}
                          value={String(source.value)}
                          onChange={(event) => {
                            const raw = event.target.value;
                            const numeric = attribute.valueType === "number" || attribute.valueType === "integer";
                            setSource(attribute.name, { value: numeric && raw !== "" ? Number(raw) : raw });
                          }}
                        />
                      )}
                    </Field>
                  ) : null}
                  {source && "longitude" in source ? (
                    <div className="grid gap-2 sm:grid-cols-2">
                      {(["longitude", "latitude"] as const).map((axis) => (
                        <Field key={axis} id={`${id}-${axis}`} label={t(`pipelines.mapper.${axis}`)}>
                          <Select
                            id={`${id}-${axis}`}
                            value={source[axis]}
                            onChange={(event) => setSource(attribute.name, { ...source, [axis]: event.target.value })}
                          >
                            {fields.map((field) => (
                              <option key={field} value={field}>
                                {field}
                              </option>
                            ))}
                          </Select>
                        </Field>
                      ))}
                    </div>
                  ) : null}
                </li>
              );
            })}
          </ul>

          {notice ? (
            <Alert role="status" tone={notice === "migrated" ? "info" : "warning"}>
              {t(`pipelines.mapper.expiry.${notice}`, { field: legacy?.field ?? "" })}
              {notice === "migrateField" && onEnds ? (
                <>
                  {" "}
                  <Button size="sm" variant="secondary" onClick={() => onEnds({ expiry, lifetime, bloblang: generate({ ...current, expires: undefined }, attributes, space) })}>
                    {t("pipelines.mapper.expiry.drop")}
                  </Button>
                </>
              ) : null}
            </Alert>
          ) : null}
          {onEnds && expiresProblem ? (
            <Alert role="status" tone="warning">
              {say(expiresProblem, "expiresAt")}
            </Alert>
          ) : null}

          <p className="text-caption text-fg-muted">
            {t("pipelines.mapper.missingAttribute")}{" "}
            <RecordLink project={project} plural="datamodels" name={typed.data.model}>
              {t("pipelines.mapper.openModel", { model: typed.data.model })}
            </RecordLink>
          </p>

          <section aria-labelledby="mapper-preview" className="flex flex-col gap-1">
            <h4 id="mapper-preview" className="text-caption font-semibold text-fg">
              {t("pipelines.mapper.preview")}
            </h4>
            <p className="text-caption text-fg-subtle">{t("pipelines.mapper.previewHint")}</p>
            {records.length === 0 ? (
              <p className="text-caption text-fg-subtle">{t("pipelines.mapper.previewEmpty")}</p>
            ) : (
              <CodeBlock
                label={t("pipelines.mapper.preview")}
                data-testid="mapper-preview"
                className="max-h-72 rounded border border-border bg-surface-subtle p-2 font-mono text-caption whitespace-pre-wrap break-words"
              >
                {JSON.stringify(preview(current, attributes, records, space, orgDomain, undefined, entityEnd), null, 2)}
              </CodeBlock>
            )}
          </section>
        </>
      ) : null}
    </div>
  );
}
