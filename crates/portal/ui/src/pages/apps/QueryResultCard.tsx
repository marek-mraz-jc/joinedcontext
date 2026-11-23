import { Fragment } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { parse as parseYaml } from "yaml";
import { Button, CodeBlock, Card, Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "../../components/ui";
import { ModelLink, TypeLink } from "../models/ModelLinks";

/**
 * What the assistant read from an endpoint through its data-plane MCP tools (AG-75): which
 * endpoint, which tool, the argument that matters, and the answer as a small table, a list of
 * fields or the text itself. The gateway wrote the answer and the model chose the call, so
 * every value is drawn as text, never as markup (AG-46).
 */

/** How many rows, attributes and characters the card draws; the step keeps the whole answer. */
export const MAX_ROWS = 10;
export const MAX_ATTRIBUTES = 5;
export const MAX_TEXT = 2000;
/** How many classes of a model the card draws, and how much of its document behind the toggle. */
export const MAX_CLASSES = 20;
export const MAX_DOCUMENT = 20_000;

/** One class of a model as a person reads it: its attributes with their type and meaning. */
export interface SchemaClass {
  name: string;
  description: string;
  attributes: { name: string; range: string; description: string }[];
}

export type QueryView =
  | { kind: "table"; columns: string[]; rows: { id: string; cells: string[] }[]; total: number }
  | { kind: "fields"; fields: [string, string][] }
  | { kind: "text"; text: string }
  | { kind: "schema"; format: string; classes: SchemaClass[]; document: string }
  | { kind: "schemaIndex"; models: { name: string; semver: string; types: string[] }[]; formats: string[]; recommended: string };

export interface QueryResult {
  endpoint: string;
  tool: string;
  argument?: string;
  view: QueryView;
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** An attribute's value as text: a normalized Property or Relationship unwrapped, keyValues as is. */
function unwrap(value: unknown): unknown {
  if (isRecord(value)) {
    if ("value" in value) {
      return value.value;
    }
    if ("object" in value) {
      return value.object;
    }
  }
  return value;
}

const GEOMETRIES = new Set([
  "Point",
  "MultiPoint",
  "LineString",
  "MultiLineString",
  "Polygon",
  "MultiPolygon",
  "GeometryCollection",
]);

/** Every `[x, y]` position under a GeoJSON value, in any nesting. */
function positionsOf(value: unknown, into: [number, number][] = []): [number, number][] {
  if (Array.isArray(value)) {
    if (typeof value[0] === "number" && typeof value[1] === "number") {
      into.push([value[0], value[1]]);
    } else {
      value.forEach((item) => positionsOf(item, into));
    }
  } else if (isRecord(value)) {
    positionsOf(value.coordinates, into);
    positionsOf(value.geometries, into);
  }
  return into;
}

const coordinate = (n: number): string => String(Math.round(n * 10_000) / 10_000);

/**
 * A GeoJSON geometry as the one line a person reads: its type and where it is, a point by its
 * position and anything wider by its bounding box. A road-work `MultiLineString` is hundreds of
 * coordinates; drawn as JSON it was thousands of digits in one cell (T-2460). The Portal may
 * already have folded it to `{ type, bbox, positions }`, which reads the same way.
 */
function geometryText(value: Record<string, unknown>): string | null {
  const kind = value.type;
  if (typeof kind !== "string" || !GEOMETRIES.has(kind)) {
    return null;
  }
  const given = Array.isArray(value.bbox) && value.bbox.length === 4 ? (value.bbox as unknown[]) : null;
  let box: number[] | null = given?.every((n) => typeof n === "number") ? (given as number[]) : null;
  if (box === null) {
    const positions = positionsOf(value);
    if (positions.length === 0) {
      return value.coordinates === undefined && value.geometries === undefined ? null : kind;
    }
    box = positions.reduce(
      ([w, s, e, n], [x, y]) => [Math.min(w, x), Math.min(s, y), Math.max(e, x), Math.max(n, y)],
      [Infinity, Infinity, -Infinity, -Infinity],
    );
  }
  const [w, s, e, n] = box.map(coordinate);
  return w === e && s === n ? `${kind} [${w}, ${s}]` : `${kind} [${w}, ${s} … ${e}, ${n}]`;
}

export function textOf(value: unknown, language?: string): string {
  const plain = unwrap(value);
  const geometry = isRecord(plain) ? geometryText(plain) : null;
  if (geometry !== null) {
    return geometry;
  }
  if (plain === null || plain === undefined) {
    return "";
  }
  if (typeof plain === "string") {
    return plain;
  }
  if (typeof plain === "number" || typeof plain === "boolean") {
    return String(plain);
  }
  // A LanguageProperty in keyValues is `{ languageMap: { en: …, fi: … } }`; a person reads the
  // one in their language, not the map (T-2760).
  if (isRecord(plain) && isRecord(plain.languageMap)) {
    const map = plain.languageMap;
    const chosen = (language !== undefined ? map[language] : undefined) ?? map.en ?? Object.values(map)[0];
    return textOf(chosen, language);
  }
  if (Array.isArray(plain)) {
    return plain.map((item) => textOf(item, language)).join(", ");
  }
  if (isRecord(plain)) {
    return Object.entries(plain)
      .map(([key, inner]) => `${key}: ${textOf(inner, language)}`)
      .join("; ");
  }
  return String(plain);
}

/** The last segment of an NGSI-LD URN, which is what a person reads as the entity's name. */
export function localId(id: string): string {
  const parts = id.split(":");
  return parts[parts.length - 1] || id;
}

const SKIPPED = new Set(["id", "type", "@context"]);

function entitiesOf(value: unknown): Record<string, unknown>[] | null {
  const list = Array.isArray(value)
    ? value
    : isRecord(value) && Array.isArray(value.entities)
      ? value.entities
      : isRecord(value) && Array.isArray(value.items)
        ? value.items
        : null;
  if (list === null || !list.every((item) => isRecord(item) && typeof item.id === "string")) {
    return null;
  }
  return list as Record<string, unknown>[];
}

/** The answer of an MCP `tools/call`: its structured content, or the JSON its text holds, or the text. */
function answerOf(output: unknown): unknown {
  if (!isRecord(output)) {
    return output;
  }
  if (output.structuredContent !== undefined) {
    return output.structuredContent;
  }
  const content = Array.isArray(output.content) ? output.content : [];
  const first = content.find((part) => isRecord(part) && typeof part.text === "string") as
    | { text: string }
    | undefined;
  if (first === undefined) {
    return output;
  }
  try {
    return JSON.parse(first.text) as unknown;
  } catch {
    return first.text;
  }
}

function fieldsOf(answer: Record<string, unknown>, language?: string): [string, string][] {
  return Object.entries(answer)
    .filter(([key]) => key !== "@context")
    .map(([key, value]) => [key, textOf(value, language)]);
}

/** `language` picks a `languageMap`'s value in the reader's language (T-2769). */
export function viewOf(output: unknown, language?: string): QueryView {
  const answer = answerOf(output);
  const entities = entitiesOf(answer);
  if (entities !== null) {
    const columns: string[] = [];
    for (const entity of entities) {
      for (const key of Object.keys(entity)) {
        if (!SKIPPED.has(key) && !columns.includes(key) && columns.length < MAX_ATTRIBUTES) {
          columns.push(key);
        }
      }
    }
    return {
      kind: "table",
      columns,
      rows: entities.slice(0, MAX_ROWS).map((entity) => ({
        id: localId(entity.id as string),
        cells: columns.map((column) => textOf(entity[column], language)),
      })),
      // A page of a longer set says the set's size, not the page's, when the answer carries it.
      total:
        isRecord(answer) && typeof answer.total === "number" && answer.total > entities.length
          ? answer.total
          : entities.length,
    };
  }
  const schema = schemaOf(answer);
  if (schema !== null) {
    return schema;
  }
  // One entity reads as its own fields, not as one field named `entity`.
  if (isRecord(answer) && isRecord(answer.entity) && typeof answer.entity.id === "string") {
    const { id, ...rest } = answer.entity;
    return { kind: "fields", fields: [["id", localId(id as string)], ...fieldsOf(rest, language)] };
  }
  if (isRecord(answer)) {
    return { kind: "fields", fields: fieldsOf(answer, language) };
  }
  const text = typeof answer === "string" ? answer : JSON.stringify(answer ?? "");
  return { kind: "text", text: text.slice(0, MAX_TEXT) };
}

/** A text of a record field, or "". */
function said(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

/**
 * A model's document as `describe_schema` answers it (`{format, document}`): a LinkML one read as
 * its classes and their attributes, any other kept as the document with its own line breaks —
 * never a JSON string of `\n` escapes (T-2769). `null` for any other answer.
 */
export function schemaOf(given: unknown): QueryView | null {
  // The gateway answers `{ schema: {format, mediaType, document} }` (its result key), beside
  // `restricted` or `jc:source` when they apply.
  const answer = isRecord(given) && isRecord(given.schema) ? given.schema : given;
  // Asked without a format, the gateway answers with the index of the formats it serves and the
  // models behind them: said in words, never as the digests and URIs it lists (T-2769).
  if (isRecord(answer) && Array.isArray(answer.artifacts) && typeof answer.document !== "string") {
    const models = (Array.isArray(answer.models) ? answer.models : []).filter(isRecord).map((model) => ({
      name: said(model.name),
      semver: said(model.semver),
      types: Array.isArray(model.types) ? model.types.filter((type): type is string => typeof type === "string") : [],
    }));
    const formats = [
      ...new Set(answer.artifacts.filter(isRecord).map((artifact) => said(artifact.format)).filter((format) => format !== "")),
    ];
    return { kind: "schemaIndex", models, formats, recommended: said(answer.recommended) };
  }
  if (!isRecord(answer) || typeof answer.document !== "string" || typeof answer.format !== "string") {
    return null;
  }
  const document = answer.document.slice(0, MAX_DOCUMENT);
  const classes: SchemaClass[] = [];
  if (answer.format === "linkml") {
    let parsed: unknown = null;
    try {
      parsed = parseYaml(answer.document) as unknown;
    } catch {
      // A document that does not parse is still shown, as the text it is.
    }
    const slots = isRecord(parsed) && isRecord(parsed.slots) ? parsed.slots : {};
    const defined = isRecord(parsed) && isRecord(parsed.classes) ? parsed.classes : {};
    for (const [name, body] of Object.entries(defined).slice(0, MAX_CLASSES)) {
      const cls = isRecord(body) ? body : {};
      const own = isRecord(cls.attributes) ? cls.attributes : {};
      const named = Array.isArray(cls.slots) ? cls.slots.filter((slot): slot is string => typeof slot === "string") : [];
      const attributes = [...Object.keys(own), ...named.filter((slot) => !Object.hasOwn(own, slot))].map((slot) => {
        const local = Object.hasOwn(own, slot) && isRecord(own[slot]) ? own[slot] : {};
        const shared = Object.hasOwn(slots, slot) && isRecord(slots[slot]) ? slots[slot] : {};
        return {
          name: slot,
          range: said(local.range) || said(shared.range),
          description: said(local.description) || said(shared.description),
        };
      });
      classes.push({ name, description: said(cls.description), attributes });
    }
  }
  return { kind: "schema", format: answer.format, classes, document };
}

/** An attribute's name as words: `createdAt` reads "created at". */
function wordsOf(column: string): string {
  return column.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase();
}

/** An ISO 8601 time as a person reads it, in their language; any other value as it is. */
export function whenOf(value: string, language?: string): string {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})$/.test(value)) {
    return value;
  }
  return new Date(value).toLocaleString(language, { dateStyle: "short", timeStyle: "short" });
}

/** The card of a `query_endpoint` step, or none when the payload is not one. */
export function queryResultOf(payload: Record<string, unknown>, language?: string): QueryResult | null {
  if (payload.tool !== "query_endpoint" || !isRecord(payload.input)) {
    return null;
  }
  if (payload.status === "failed" || payload.error !== undefined || payload.output === undefined) {
    return null;
  }
  const input = payload.input;
  const endpoint = typeof input.endpoint === "string" ? input.endpoint : "";
  const tool = typeof input.name === "string" ? input.name : "";
  const args = isRecord(input.arguments) ? input.arguments : {};
  const key = ["type", "id", "q"].find((name) => typeof args[name] === "string" && args[name] !== "");
  return {
    endpoint,
    tool,
    argument: key === undefined ? undefined : `${key}: ${key === "id" ? localId(String(args[key])) : String(args[key])}`,
    view: viewOf(payload.output, language),
  };
}

export function QueryResultCard({
  result,
  project,
  onAsk,
}: {
  result: QueryResult;
  project?: string;
  /** Sends a message for the person: what an empty read offers. */
  onAsk?: (text: string) => void;
}): JSX.Element {
  return (
    <Card padding="xs" data-testid="query-result" className="text-xs">
      <p className="flex flex-wrap items-baseline gap-x-2 font-medium">
        <span>{result.endpoint}</span>
        {result.argument !== undefined ? (
          <span className="font-mono text-fg-muted">{result.argument}</span>
        ) : null}
      </p>
      <QueryAnswer view={result.view} project={project} onAsk={onAsk} />
    </Card>
  );
}

/** An answer as a small table of entities, a list of fields or the text itself. */
/**
 * One answer's view. Given the `project`, a model or a type it names links to the model's page
 * (T-2766); without it (a change test's sample) the names stay text.
 */
export function QueryAnswer({
  view,
  project,
  onAsk,
}: {
  view: QueryView;
  project?: string;
  onAsk?: (text: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  return view.kind === "table" && view.total === 0 ? (
    <div className="mt-1 flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
      <div className="min-w-0">
        <p className="font-medium">{t("assistant.query.nothing")}</p>
        <p className="text-fg-muted">{t("assistant.query.nothingHint")}</p>
      </div>
      {onAsk ? (
        <Button size="sm" variant="secondary" onClick={() => onAsk(t("assistant.query.widenAsk"))}>
          {t("assistant.query.widen")}
        </Button>
      ) : null}
    </div>
  ) : view.kind === "table" ? (
    <div className="@container">
      {/* The shared Table, not a hand-made one: the header scope, the caption a screen reader
          reads, and the sideways scroll a keyboard can drive all come from one place (UI-01,
          UI-27). The dock is narrow at every screen width, so the table stacks by its own width:
          each row a card, every row and column in reach in the conversation's own scroll. */}
      <Table
        caption={t("assistant.query.caption")}
        zebra={false}
        className="table-stack-narrow mt-1 text-caption"
      >
        <TableHead>
          <TableHeaderCell className="px-2 py-1">{t("assistant.query.id")}</TableHeaderCell>
          {view.columns.map((column) => (
            <TableHeaderCell key={column} className="px-2 py-1">
              {wordsOf(column)}
            </TableHeaderCell>
          ))}
        </TableHead>
        <TableBody>
          {view.rows.map((row, index) => (
            <TableRow key={`${row.id}-${index}`}>
              <TableCell className="px-2 py-1 font-mono">{row.id}</TableCell>
              {row.cells.map((cell, cellIndex) => (
                <TableCell key={view.columns[cellIndex]} className="px-2 py-1" title={cell} data-label={wordsOf(view.columns[cellIndex])}>
                  {/* A table cell takes no max-width, so the fold is on a block inside it: a
                      4 000-character value used to widen its column to 4 000 characters and
                      push every other column off the card (T-2460). */}
                  <span className="block max-w-48 truncate">{whenOf(cell, i18n.resolvedLanguage)}</span>
                </TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <p className="mt-1 text-fg-muted">
        {view.total > view.rows.length ? (
          <>
            <span>{t("assistant.query.someRows", { shown: view.rows.length, total: view.total })}</span>
            {" · "}
            <span>{t("assistant.query.moreHint")}</span>
          </>
        ) : (
          t("assistant.query.rows", { count: view.total })
        )}
      </p>
    </div>
  ) : view.kind === "schema" ? (
    <SchemaAnswer view={view} project={project} />
  ) : view.kind === "schemaIndex" ? (
    <ul className="mt-1 flex flex-col gap-0.5">
      {view.models.map((model) => (
        <li key={`${model.name}@${model.semver}`} className="break-words">
          {project === undefined ? (
            t("assistant.query.model", {
              types: model.types.join(", "),
              name: model.name,
              semver: model.semver,
            })
          ) : (
            <>
              {model.types.map((type, index) => (
                <Fragment key={type}>
                  {index === 0 ? null : ", "}
                  <TypeLink project={project} type={type} className="font-mono" />
                </Fragment>
              ))}
              {model.types.length === 0 ? null : ", "}
              {t("assistant.query.fromModel")} <ModelLink project={project} name={model.name} className="font-mono" />{" "}
              {model.semver}
            </>
          )}
        </li>
      ))}
      <li className="break-words text-fg-muted">
        {t("assistant.query.formats", {
          formats: view.formats.join(", "),
          recommended: view.recommended,
        })}
      </li>
    </ul>
  ) : view.kind === "fields" ? (
    <dl className="mt-1 grid grid-cols-[auto_1fr] gap-x-2 gap-y-0.5">
      {view.fields.map(([key, value]) => (
        <div key={key} className="contents">
          <dt className="text-fg-muted">{wordsOf(key)}</dt>
          <dd className="min-w-0 break-words">{whenOf(value, i18n.resolvedLanguage)}</dd>
        </div>
      ))}
    </dl>
  ) : (
    <CodeBlock label={t("codeBlock.answer")} className="mt-1 max-h-48 whitespace-pre-wrap break-words font-mono">{view.text}</CodeBlock>
  );
}

/** A model's classes as small tables, and its document behind a toggle (T-2769). */
function SchemaAnswer({
  view,
  project,
}: {
  view: Extract<QueryView, { kind: "schema" }>;
  project?: string;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <>
      {view.classes.map((cls) => (
        <section key={cls.name} className="mt-1">
          <p className="break-words">
            {project === undefined ? (
              <span className="font-medium">{cls.name}</span>
            ) : (
              <TypeLink project={project} type={cls.name} className="font-medium" />
            )}
            {cls.description !== "" ? <span className="text-fg-muted"> · {cls.description}</span> : null}
          </p>
          {cls.attributes.length > 0 ? (
            <Table
              caption={t("assistant.query.attributes", { name: cls.name })}
              zebra={false}
              maxHeight="max-h-48"
              className="mt-0.5 text-caption"
            >
              <TableHead>
                <TableHeaderCell className="px-2 py-1">{t("assistant.query.attribute")}</TableHeaderCell>
                <TableHeaderCell className="px-2 py-1">{t("assistant.query.range")}</TableHeaderCell>
                <TableHeaderCell className="px-2 py-1">{t("assistant.query.meaning")}</TableHeaderCell>
              </TableHead>
              <TableBody>
                {cls.attributes.map((attribute) => (
                  <TableRow key={attribute.name}>
                    <TableCell className="px-2 py-1 font-mono">{attribute.name}</TableCell>
                    <TableCell className="px-2 py-1 font-mono">{attribute.range}</TableCell>
                    <TableCell className="px-2 py-1">
                      <span className="block max-w-64 break-words">{attribute.description}</span>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          ) : null}
        </section>
      ))}
      <details className="mt-1" open={view.classes.length === 0}>
        <summary className="cursor-pointer text-fg-muted">
          {t("assistant.query.document", { format: view.format })}
        </summary>
        <CodeBlock label={t("assistant.query.document", { format: view.format })} className="mt-1 max-h-48 whitespace-pre-wrap break-words font-mono">{view.document}</CodeBlock>
      </details>
    </>
  );
}
