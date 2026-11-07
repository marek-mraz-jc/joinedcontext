import { createContext, useCallback, useContext, useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { columnKind, format } from "../ngsi";
import { optionLabel } from "../enums";
import type { Cell, Row } from "../ngsi";
import { fieldOf } from "../write";
import type { Field, FieldSchema, Schema, TypeSchema } from "../write";
import { endpointsOf, ProblemError } from "./client";
import { unitSymbol } from "./units";
import { useAccess, useClient, useMe, useSchema } from "./hooks";
import { sdkLanguage, sdkWord, type SdkLanguage, type SdkWord } from "./words";

/**
 * The entity panel every App shares (SDK-40): a map feature, a table row, a chart point or a card
 * calls `select`, and the one panel of the shell shows that entity. Edit is offered only where the
 * reader's own access document allows the write; the change is checked against the schema and
 * shown before it is written through the App's Endpoint with the reader's session. Otherwise the
 * panel links to the entity in the Portal, which applies the reader's own rights.
 */

export interface SelectedEntity {
  id: string;
  type: string;
  /** The endpoint the type is read and written through, in an App reading several. */
  endpoint?: string;
}

/**
 * Where the panel reads and writes when the App does not go through the SDK's client: a `ui-rust`
 * App's own backend, which holds the endpoint so the browser never reaches it (AP-04). The App
 * answers what the reader may change from what its backend knows of the reader's roles; the
 * backend and the gateway still decide every write.
 */
export interface PanelSource {
  /** One entity, read fresh; a `ProblemError` with status 404 says it is gone. */
  get(entity: SelectedEntity): Promise<Row>;
  /** Writes the changed attributes; a refusal is a `ProblemError` with the endpoint's status. */
  update(entity: SelectedEntity, patch: Record<string, Cell>): Promise<void>;
  /** Whether the reader may change `attr` of `type`, or any attribute of it without one. */
  mayEdit(type: string, attr?: string): boolean;
  /** The entity's page in the Portal, when the reader should change it there instead. */
  portalLink?(entity: SelectedEntity): string | null;
  /** The App's schema, for the labels and the checks of a change. */
  schema?: Schema;
  language?: string;
}

export interface Selection {
  /** Whether a shell holds the panel: outside one, a grid keeps its own row detail. */
  inShell: boolean;
  source?: PanelSource;
  /** The language the App's shell speaks now, which the panel speaks too. */
  language?: string;
  selected: SelectedEntity | null;
  select(entity: SelectedEntity): void;
  clear(): void;
  /** How many changes the panel has saved: a page that lists the entities reads them again when it moves. */
  saved: number;
  /** Counts one saved change; the panel calls it. */
  markSaved(): void;
}

const SelectionContext = createContext<Selection | null>(null);

/** Holds what is selected and remembers what opened it, so closing gives the focus back. */
export function EntitySelectionProvider({ children, source, language }: { children?: ReactNode; source?: PanelSource; language?: string }): React.JSX.Element {
  const [selected, setSelected] = useState<SelectedEntity | null>(null);
  const [saved, setSaved] = useState(0);
  const markSaved = useCallback(() => setSaved((count) => count + 1), []);
  const opener = useRef<HTMLElement | null>(null);
  const select = useCallback((entity: SelectedEntity) => {
    if (typeof document !== "undefined" && document.activeElement instanceof HTMLElement) {
      opener.current = document.activeElement;
    }
    setSelected(entity);
  }, []);
  const clear = useCallback(() => {
    setSelected(null);
    const back = opener.current;
    opener.current = null;
    // After the panel is gone, so the focus lands on what opened it and not on nothing.
    if (back && typeof window !== "undefined") window.setTimeout(() => back.isConnected && back.focus(), 0);
  }, []);
  const value = useMemo(
    () => ({ inShell: true, source, language, selected, select, clear, saved, markSaved }),
    [source, language, selected, select, clear, saved, markSaved],
  );
  return <SelectionContext.Provider value={value}>{children}</SelectionContext.Provider>;
}

/** What is selected, and how to select an entity or close the panel; outside the shell it does nothing. */
export function useEntitySelection(): Selection {
  return useContext(SelectionContext) ?? NO_SELECTION;
}

const NO_SELECTION: Selection = { inShell: false, selected: null, select: () => undefined, clear: () => undefined, saved: 0, markSaved: () => undefined };

/** Props that make any element open the panel on a click and on Enter or Space. */
export function selectable(entity: SelectedEntity, select: (entity: SelectedEntity) => void): {
  role: "button";
  tabIndex: 0;
  onClick: () => void;
  onKeyDown: (event: { key: string; preventDefault(): void }) => void;
} {
  return {
    role: "button",
    tabIndex: 0,
    onClick: () => select(entity),
    onKeyDown: (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        select(entity);
      }
    },
  };
}

/** The attributes a person reads, in the order the schema names them, then the rest. */
export function attributeOrder(row: Row, properties: Record<string, FieldSchema> | undefined): string[] {
  const own = Object.keys(row).filter((name) => name !== "id" && name !== "type" && name !== "@context");
  const ordered = Object.keys(properties ?? {}).filter((name) => own.includes(name));
  return [...ordered, ...own.filter((name) => !ordered.includes(name))];
}

/** An attribute's label: the schema's `title` where it has one, else its name in words. */
export function labelOf(name: string, property: FieldSchema | undefined): string {
  const title = property?.title;
  if (title && title.trim() !== "") return title;
  const words = name.replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ").toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** The inputs the panel edits in place; a geometry, a relationship or a language map is the Portal's. */
const EDITABLE = new Set(["text", "number", "select", "date", "checkbox"]);

/** A typed value checked against the schema, or the sentence that says why not. */
export function parseValue(field: Field, text: string, language: SdkLanguage): { value: Cell } | { error: string } {
  const word = (key: SdkWord, slots?: Record<string, string | number>) => sdkWord(language, key, slots);
  if (text.trim() === "") {
    return field.required ? { error: word("form.required") } : { value: null };
  }
  if (field.input === "number") {
    const number = Number(text.replace(",", "."));
    if (!Number.isFinite(number)) return { error: word("form.number") };
    if (field.min !== undefined && number < field.min) return { error: word("form.atLeast", { min: field.min }) };
    if (field.max !== undefined && number > field.max) return { error: word("form.atMost", { max: field.max }) };
    return { value: number };
  }
  if (field.input === "checkbox") return { value: text === "true" };
  if (field.input === "select" && field.options && !field.options.some((option) => option.value === text)) {
    return { error: word("form.oneOf", { options: field.options.map(optionLabel).join(", ") }) };
  }
  if (field.pattern) {
    try {
      if (!new RegExp(`^(?:${field.pattern})$`).test(text)) return { error: word("form.pattern") };
    } catch {
      // A pattern the browser cannot compile checks nothing; the endpoint still validates.
    }
  }
  return { value: text };
}

function draftOf(value: Cell, field: Field): string {
  if (value === null || value === undefined) return "";
  if (field.input === "checkbox") return value === true ? "true" : "false";
  // The stored number, every digit: a grouped `2,024` would parse back as 2.024 (T-3600).
  if (typeof value === "number") return String(value);
  return format(value);
}

function shown(value: Cell, language: SdkLanguage, unit?: string): string {
  if (value === null || value === undefined || value === "") return sdkWord(language, "panel.empty");
  if (value === true) return sdkWord(language, "panel.yes");
  if (value === false) return sdkWord(language, "panel.no");
  // As the grid writes it (T-3600): every digit, no grouping, so a year stays `2024`; then the
  // model's unit, `15.176 µg/m³`.
  if (typeof value === "number" && Number.isFinite(value)) {
    const symbol = unitSymbol(unit);
    return symbol ? `${value} ${symbol}` : String(value);
  }
  return format(value);
}

/** The entity's page in the Portal: the explorer of the App's project, opened on it. */
export function portalLinkOf(portal: string | undefined, space: string, id: string): string | null {
  if (!portal) return null;
  return `${portal}/explore?space=${encodeURIComponent(space)}&entityId=${encodeURIComponent(id)}`;
}

type Stage = { kind: "view" } | { kind: "edit" } | { kind: "review"; patch: Record<string, Cell> } | { kind: "saving"; patch: Record<string, Cell> };

/** The one panel of the shell, showing the selected entity; nothing while nothing is selected. */
export function EntityPanel(): React.JSX.Element | null {
  const { selected, source } = useEntitySelection();
  if (!selected) return null;
  return source ? <SourcePanel key={selected.id} entity={selected} source={source} /> : <ClientPanel key={selected.id} entity={selected} />;
}

/** What the panel works from, whichever way the App reads its entities. */
interface Backing {
  load(): Promise<Row>;
  save(patch: Record<string, Cell>): Promise<void>;
  /** Whether the reader may change `attr`, or any attribute without one. */
  mayEdit(attr?: string): boolean;
  portal: string | null;
  typeSchema: TypeSchema | null;
  defs: Schema | null;
  language: SdkLanguage;
}

/** The panel of an App that reads through the SDK's client: the reader's own access document decides Edit. */
function ClientPanel({ entity }: { entity: SelectedEntity }): React.JSX.Element {
  const client = useClient();
  const { language: selectionLanguage } = useEntitySelection();
  const user = useMe();
  const { schema, typeSchema } = useSchema(entity.type);
  const { can } = useAccess(entity.endpoint);
  const signedIn = user !== null && user !== undefined;
  const backing: Backing = {
    load: () => client.entities.get(entity.id, undefined, { endpoint: entity.endpoint }),
    save: (patch) => client.entities.update(entity.id, patch, { endpoint: entity.endpoint }),
    mayEdit: (attr) => signedIn && can("updateAttrs", entity.type, attr).ok,
    // The space the entity was read from: a named endpoint may serve another project's space.
    portal: portalLinkOf(client.config.portal, endpointsOf(client.config).find((one) => one.name === entity.endpoint)?.space ?? client.config.space, entity.id),
    typeSchema,
    defs: schema,
    // The shell's language first: an App that switches languages gets its panel switched too.
    language: sdkLanguage(selectionLanguage ?? client.config.language),
  };
  return <PanelView entity={entity} backing={backing} />;
}

/** The panel of an App that reads through its own backend (`PanelSource`). */
function SourcePanel({ entity, source }: { entity: SelectedEntity; source: PanelSource }): React.JSX.Element {
  const backing: Backing = {
    load: () => source.get(entity),
    save: (patch) => source.update(entity, patch),
    mayEdit: (attr) => source.mayEdit(entity.type, attr),
    portal: source.portalLink?.(entity) ?? null,
    typeSchema: source.schema?.[entity.type] ?? null,
    defs: source.schema ?? null,
    language: sdkLanguage(source.language),
  };
  return <PanelView entity={entity} backing={backing} />;
}

function PanelView({ entity, backing }: { entity: SelectedEntity; backing: Backing }): React.JSX.Element {
  const { clear, markSaved } = useEntitySelection();
  const { language, typeSchema, defs } = backing;
  const schema = defs;
  const word = (key: SdkWord, slots?: Record<string, string | number>) => sdkWord(language, key, slots);
  const headingId = useId();
  const heading = useRef<HTMLHeadingElement>(null);

  const [row, setRow] = useState<Row | null>(null);
  const [readError, setReadError] = useState<ProblemError | null>(null);
  const [gone, setGone] = useState(false);
  const [stage, setStage] = useState<Stage>({ kind: "view" });
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [notice, setNotice] = useState<{ tone: "ok" | "problem"; text: string } | null>(null);

  const { load } = backing;
  const loadRef = useRef(load);
  loadRef.current = load;
  // Read once per entity: the backing is rebuilt each render, the entity is what changes.
  const read = useCallback(async () => {
    try {
      setRow(await loadRef.current());
      setReadError(null);
    } catch (err) {
      if (err instanceof ProblemError && err.status === 404) {
        setGone(true);
      } else {
        setReadError(err instanceof ProblemError ? err : new ProblemError(0, { title: err instanceof Error ? err.message : String(err) }));
      }
    }
  }, []);

  useEffect(() => {
    void read();
  }, [read]);

  useEffect(() => {
    heading.current?.focus();
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") clear();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [clear]);

  const properties = typeSchema?.properties;
  // An attribute the schema names and the entity does not hold yet, shown only to a reader who
  // may write it, so a first value can be filled in (a note never written, T-3422).
  const mayWrite = backing.mayEdit();
  const fillable = row && mayWrite ? Object.keys(properties ?? {}).filter((name) => !(name in row) && backing.mayEdit(name)) : [];
  const fillableKey = fillable.join(",");
  const { names, fields } = useMemo(() => {
    const attrs = row ? [...attributeOrder(row, typeSchema?.properties), ...(fillableKey ? fillableKey.split(",") : [])] : [];
    const map: Record<string, Field> = {};
    for (const name of attrs) {
      map[name] = fieldOf(name, typeSchema ?? undefined, columnKind(row ? [row] : [], name), schema ?? undefined, language);
    }
    return { names: attrs, fields: map };
  }, [row, typeSchema, schema, language, fillableKey]);

  // Edit only where the reader may write this type; the attributes they may not change stay
  // read-only in the form.
  const editable = (name: string) => mayWrite && EDITABLE.has(fields[name]?.input ?? "") && backing.mayEdit(name);
  const portal = backing.portal;

  const startEdit = () => {
    if (!row) return;
    setDraft(Object.fromEntries(names.filter(editable).map((name) => [name, draftOf(row[name], fields[name])])));
    setErrors({});
    setNotice(null);
    setStage({ kind: "edit" });
  };

  const review = () => {
    if (!row) return;
    const patch: Record<string, Cell> = {};
    const found: Record<string, string> = {};
    for (const [name, text] of Object.entries(draft)) {
      // Only what the reader changed is checked: a value left as it was is not theirs to fix.
      if (text === draftOf(row[name], fields[name])) continue;
      const parsed = parseValue(fields[name], text, language);
      if ("error" in parsed) {
        found[name] = parsed.error;
      } else if (draftOf(parsed.value, fields[name]) !== draftOf(row[name], fields[name])) {
        patch[name] = parsed.value;
      }
    }
    setErrors(found);
    if (Object.keys(found).length > 0) return;
    if (Object.keys(patch).length === 0) {
      setNotice({ tone: "ok", text: word("panel.noChange") });
      setStage({ kind: "view" });
      return;
    }
    setStage({ kind: "review", patch });
  };

  const save = async (patch: Record<string, Cell>) => {
    setStage({ kind: "saving", patch });
    try {
      await backing.save(patch);
      markSaved();
      await read();
      setNotice({ tone: "ok", text: word("panel.saved") });
      setStage({ kind: "view" });
    } catch (err) {
      const problem = err instanceof ProblemError ? err : new ProblemError(0, { title: err instanceof Error ? err.message : String(err) });
      const reason = problem.detail || problem.title;
      if (problem.status === 409) {
        await read();
        setNotice({ tone: "problem", text: word("panel.conflict") });
        setStage({ kind: "edit" });
      } else if (problem.status === 403) {
        setNotice({ tone: "problem", text: word("panel.forbidden", { reason }) });
        setStage({ kind: "view" });
      } else {
        setNotice({ tone: "problem", text: word("panel.failed", { reason }) });
        setStage({ kind: "review", patch });
      }
    }
  };

  const title = row ? format(row.name ?? null) || entity.id.slice(entity.id.lastIndexOf(":") + 1) : entity.id.slice(entity.id.lastIndexOf(":") + 1);

  return (
    <div className="jc-panel" role="dialog" aria-modal="false" aria-labelledby={headingId}>
      <div className="jc-panel-header">
        <h2 id={headingId} ref={heading} tabIndex={-1}>
          {title}
        </h2>
        {/* `data-jc-sdk`: the close button is the SDK's to test in every language, not each App's (T-4113). */}
        <button type="button" className="jc-panel-close" onClick={clear} aria-label={word("panel.close")} data-jc-sdk="panel">
          ×
        </button>
      </div>
      <p className="jc-panel-type">{entity.type}</p>

      {notice && (
        <p className={notice.tone === "ok" ? "jc-panel-ok" : "jc-panel-problem"} role={notice.tone === "ok" ? "status" : "alert"}>
          {notice.text}
        </p>
      )}

      {gone ? (
        <p>{word("panel.gone")}</p>
      ) : readError ? (
        <div className="jc-panel-problem" role="alert">
          <p>{readError.detail || readError.title}</p>
          <button type="button" onClick={() => void read()}>
            {word("state.retry")}
          </button>
        </div>
      ) : !row ? (
        <p role="status">{word("panel.reading")}</p>
      ) : stage.kind === "view" ? (
        <>
          <dl className="jc-panel-attrs">
            {names.map((name) => (
              <div key={name}>
                <dt title={properties?.[name]?.description}>{labelOf(name, properties?.[name])}</dt>
                <dd>{shown(row[name], language, fields[name]?.unit)}</dd>
              </div>
            ))}
          </dl>
          <div className="jc-panel-actions">
            {mayWrite && names.some(editable) ? (
              <button type="button" onClick={startEdit}>
                {word("panel.edit")}
              </button>
            ) : portal ? (
              <>
                <a href={portal} target="_blank" rel="noopener noreferrer">
                  {word("panel.portal")}
                </a>
                <p className="jc-panel-hint">{word("panel.portalHint")}</p>
              </>
            ) : null}
          </div>
        </>
      ) : stage.kind === "edit" ? (
        <form
          className="jc-panel-form"
          onSubmit={(event) => {
            event.preventDefault();
            review();
          }}
        >
          {names.map((name) => {
            const field = fields[name];
            const label = labelOf(name, properties?.[name]);
            if (!(name in draft)) {
              return (
                <div key={name} className="jc-panel-field">
                  <span className="jc-panel-label">{label}</span>
                  <span>
                    {shown(row[name], language, fields[name]?.unit)} <small>({word("panel.inPortal")})</small>
                  </span>
                </div>
              );
            }
            const id = `${headingId}-${name}`;
            const error = errors[name];
            const describedBy = error ? `${id}-error` : undefined;
            return (
              <div key={name} className="jc-panel-field">
                <label htmlFor={id}>{label}</label>
                {field.input === "select" && field.options ? (
                  <select id={id} value={draft[name]} aria-invalid={error ? true : undefined} aria-describedby={describedBy} onChange={(event) => setDraft({ ...draft, [name]: event.target.value })}>
                    {!field.required && <option value="">{word("panel.empty")}</option>}
                    {field.options.map((option) => (
                      <option key={option.value} value={option.value}>
                        {optionLabel(option)}
                      </option>
                    ))}
                  </select>
                ) : field.input === "checkbox" ? (
                  <input id={id} type="checkbox" checked={draft[name] === "true"} onChange={(event) => setDraft({ ...draft, [name]: event.target.checked ? "true" : "false" })} />
                ) : (
                  <input
                    id={id}
                    type="text"
                    inputMode={field.input === "number" ? "decimal" : undefined}
                    value={draft[name]}
                    aria-invalid={error ? true : undefined}
                    aria-describedby={describedBy}
                    onChange={(event) => setDraft({ ...draft, [name]: event.target.value })}
                  />
                )}
                {error && (
                  <span id={`${id}-error`} className="jc-panel-error">
                    {error}
                  </span>
                )}
              </div>
            );
          })}
          <div className="jc-panel-actions">
            <button type="submit">{word("panel.review")}</button>
            <button type="button" onClick={() => setStage({ kind: "view" })}>
              {word("panel.cancel")}
            </button>
          </div>
        </form>
      ) : (
        <div className="jc-panel-review">
          <p>{word("panel.changes")}</p>
          <ul>
            {Object.entries(stage.patch).map(([name, value]) => (
              <li key={name}>
                {word("panel.change", { attr: labelOf(name, properties?.[name]), from: shown(row[name], language, fields[name]?.unit), to: shown(value, language, fields[name]?.unit) })}
              </li>
            ))}
          </ul>
          <div className="jc-panel-actions">
            <button type="button" disabled={stage.kind === "saving"} onClick={() => void save(stage.patch)}>
              {stage.kind === "saving" ? word("panel.saving") : word("panel.confirm")}
            </button>
            <button type="button" disabled={stage.kind === "saving"} onClick={() => setStage({ kind: "edit" })}>
              {word("panel.back")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
