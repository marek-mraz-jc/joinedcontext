/**
 * The row dialog's Changes tab (T-6333): who changed which attribute of one entity, from what to
 * what, when, through what and why, newest first, from the versions the broker keeps (ADR-N-054
 * 2.8, 2.9). The broker decides what the caller reads: a caller without `readVersions` gets a 403,
 * which hides the tab and the marks, never a message with the server's words.
 */
import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { actorName, changesCsv, filterChanges } from "./changes";
import type { EntityChange } from "./changes";
import { SourceError } from "./source";
import type { EntitySource } from "./source";
import { toRichCell } from "./model";
import type { RichCell } from "./model";
import "./grid.css";

export interface ChangesLabels {
  /** The tab: "Changes". */
  tab: string;
  attribute: string;
  person: string;
  /** The filter entry that shows every attribute or every person. */
  all: string;
  before: string;
  after: string;
  channel: string;
  organization: string;
  reason: string;
  /** A value the version did not have, such as an attribute added by the change. */
  noValue: string;
  empty: string;
  /** No change matches the chosen attribute and person. */
  emptyFiltered: string;
  /** The button under that sentence that clears both filters. */
  showAll: string;
  loading: string;
  error: string;
  retry: string;
  copy: string;
  copied: string;
  /** The mark beside an attribute with kept versions, and its accessible name ("{attr}" filled). */
  changed: string;
  showChanges: string;
  /** How a writer is called by its kind (person, pipeline, serviceAccount, agent). */
  actorKinds: Record<string, string>;
}

export const DEFAULT_CHANGES_LABELS: ChangesLabels = {
  tab: "Changes",
  attribute: "Attribute",
  person: "Changed by",
  all: "All",
  before: "Before",
  after: "After",
  channel: "Through",
  organization: "Organization",
  reason: "Reason",
  noValue: "(no value)",
  empty:
    "No change of this row is kept yet. An edit made with a reason from now on shows here.",
  emptyFiltered: "No change matches these filters.",
  showAll: "Show every change",
  loading: "Loading changes…",
  error:
    "The changes could not be read. The service did not answer; try again in a moment.",
  retry: "Try again",
  copy: "Copy as CSV",
  copied: "Copied",
  changed: "changed",
  showChanges: "Show the changes of {attr}",
  actorKinds: {
    person: "person",
    pipeline: "pipeline",
    serviceAccount: "service account",
    agent: "agent",
  },
};

export type ChangesState =
  /** `again`: a Try again is reading, so the tab it was pressed in stays. */
  | { status: "loading"; again?: boolean }
  | { status: "refused" }
  | { status: "error"; retry: () => void }
  | { status: "ready"; changes: EntityChange[] };

/** The tab shows once the first read answered: a refused caller never sees it appear and go. */
export function changesShown(state: ChangesState | null): boolean {
  return (
    state !== null &&
    (state.status === "ready" ||
      state.status === "error" ||
      (state.status === "loading" && state.again === true))
  );
}

/**
 * One read of `attrs`' changes per entity: a 403 is a refusal the dialog hides, a 404 is nothing
 * kept (the source answers []), anything else a failure with Try again.
 */
export function useEntityChanges(
  load: ((attrs: string[]) => Promise<EntityChange[]>) | undefined,
  id: string,
  attrs: string[],
): ChangesState | null {
  const key = attrs.join(",");
  const [state, setState] = useState<ChangesState>({ status: "loading" });
  const [round, setRound] = useState(0);
  const again = useRef(false);
  const retry = useCallback(() => {
    again.current = true;
    setRound((n) => n + 1);
  }, []);
  useEffect(() => {
    if (!load || key === "") return;
    let live = true;
    setState({ status: "loading", again: again.current });
    again.current = false;
    load(key.split(",")).then(
      (changes) => live && setState({ status: "ready", changes }),
      (error: unknown) =>
        live &&
        setState(
          error instanceof SourceError && error.status === 403
            ? { status: "refused" }
            : { status: "error", retry },
        ),
    );
    return () => {
      live = false;
    };
  }, [load, id, key, round, retry]);
  return load && key !== "" ? state : null;
}

export interface EntityChangesProps {
  state: ChangesState;
  /** The attribute filter, owned by the dialog so a field's mark can open the tab on it. */
  attr: string | null;
  onAttr: (attr: string | null) => void;
  /** The attributes as the grid labels them, in its order. */
  attrs: { attr: string; label: string }[];
  /** One kept instance drawn by the grid's own cell renderer, or null when it has no value. */
  renderValue: (
    attr: string,
    instance: Record<string, unknown>,
  ) => React.ReactNode;
  locale: string;
  labels?: Partial<ChangesLabels>;
}

export function EntityChanges({
  state,
  attr,
  onAttr,
  attrs,
  renderValue,
  locale,
  labels: given,
}: EntityChangesProps): React.JSX.Element {
  const labels = { ...DEFAULT_CHANGES_LABELS, ...given };
  const [actor, setActor] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timer);
  }, [copied]);
  const all = state.status === "ready" ? state.changes : [];
  const shown = useMemo(
    () => filterChanges(all, attr, actor),
    [all, attr, actor],
  );
  const labelOf = (name: string) =>
    attrs.find((one) => one.attr === name)?.label ?? name;
  const changedAttrs = [...new Set(all.map((change) => change.attr))];
  const actors = [
    ...new Set(
      all
        .map((change) => change.provenance.actor)
        .filter((one): one is string => !!one),
    ),
  ];
  // Two writers with one short name (user:jana and serviceAccount:jana) are told apart by their full id.
  const optionName = (one: string) => {
    const name = actorName({ actor: one });
    return actors.filter((other) => actorName({ actor: other }) === name)
      .length > 1
      ? one
      : name;
  };
  const stamp = (at: string) =>
    new Date(at).toLocaleString(locale, {
      dateStyle: "medium",
      timeStyle: "short",
    });
  const value = (name: string, instance: Record<string, unknown>) =>
    renderValue(name, instance) ?? (
      <span className="jc-grid-detail-empty">{labels.noValue}</span>
    );

  if (state.status === "loading")
    return (
      <p className="jc-grid-changes-state" role="status">
        {labels.loading}
      </p>
    );
  if (state.status === "error") {
    return (
      <div
        className="jc-grid-changes-state jc-grid-changes-failed"
        role="alert"
      >
        <p>{labels.error}</p>
        <button type="button" onClick={state.retry}>
          {labels.retry}
        </button>
      </div>
    );
  }
  if (all.length === 0)
    return <p className="jc-grid-changes-state">{labels.empty}</p>;

  return (
    <section className="jc-grid-changes" aria-label={labels.tab}>
      <div className="jc-grid-changes-filters">
        <label>
          {labels.attribute}
          <select
            value={attr ?? ""}
            onChange={(e) => onAttr(e.target.value || null)}
          >
            <option value="">{labels.all}</option>
            {changedAttrs.map((name) => (
              <option key={name} value={name}>
                {labelOf(name)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {labels.person}
          <select
            value={actor ?? ""}
            onChange={(e) => setActor(e.target.value || null)}
          >
            <option value="">{labels.all}</option>
            {actors.map((one) => (
              <option key={one} value={one}>
                {optionName(one)}
              </option>
            ))}
          </select>
        </label>
        {/* Nothing to copy when the filters match nothing: the button goes, the sentence below says why. */}
        {shown.length > 0 ? (
          <button
            type="button"
            onClick={() =>
              void navigator.clipboard?.writeText(changesCsv(shown)).then(
                () => setCopied(true),
                () => setCopied(false),
              )
            }
          >
            {copied ? labels.copied : labels.copy}
          </button>
        ) : null}
        <span className="jc-grid-sr-only" role="status">
          {copied ? labels.copied : ""}
        </span>
      </div>
      {shown.length === 0 ? (
        <div className="jc-grid-changes-state">
          <p>{labels.emptyFiltered}</p>
          <button
            type="button"
            onClick={() => {
              onAttr(null);
              setActor(null);
            }}
          >
            {labels.showAll}
          </button>
        </div>
      ) : (
        <ol className="jc-grid-changes-list">
          {shown.map((change, index) => {
            const p = change.provenance;
            const kind = p.actorKind
              ? (labels.actorKinds[p.actorKind] ?? p.actorKind)
              : "";
            return (
              <li
                key={`${change.attr}-${change.datasetId ?? ""}-${change.at}-${index}`}
              >
                <div className="jc-grid-changes-head">
                  <strong>{labelOf(change.attr)}</strong>
                  <time dateTime={change.at}>{stamp(change.at)}</time>
                </div>
                <dl className="jc-grid-changes-values">
                  <div>
                    <dt>{labels.before}</dt>
                    <dd>{value(change.attr, change.before)}</dd>
                  </div>
                  <div>
                    <dt>{labels.after}</dt>
                    <dd>{value(change.attr, change.after)}</dd>
                  </div>
                </dl>
                <p className="jc-grid-changes-who">
                  {p.actor ? (
                    <span>
                      {labels.person}: <strong>{actorName(p)}</strong>
                      {kind ? ` (${kind})` : ""}
                    </span>
                  ) : null}
                  {p.organization ? (
                    <span>{`${labels.organization}: ${p.organization}`}</span>
                  ) : null}
                  {p.channel ? (
                    <span>{`${labels.channel}: ${p.channel}`}</span>
                  ) : null}
                </p>
                {p.reason ? (
                  <p className="jc-grid-changes-reason">
                    {labels.reason}: {p.reason}
                  </p>
                ) : null}
              </li>
            );
          })}
        </ol>
      )}
    </section>
  );
}

/** What a row dialog gives the Changes tab: the read, how a value is drawn, and the words. */
export interface RowChanges {
  load: (attrs: string[]) => Promise<EntityChange[]>;
  renderValue: (
    attr: string,
    instance: Record<string, unknown>,
  ) => React.ReactNode;
  locale: string;
  labels?: Partial<ChangesLabels>;
}

/** The pill beside a field with kept versions: it opens Changes on that field. */
export function ChangedMark({
  label,
  labels,
  onOpen,
}: {
  label: string;
  labels?: Partial<ChangesLabels>;
  onOpen: () => void;
}): React.JSX.Element {
  const words = { ...DEFAULT_CHANGES_LABELS, ...labels };
  return (
    <button
      type="button"
      className="jc-grid-changed-mark"
      aria-label={words.showChanges.replace("{attr}", label)}
      onClick={onOpen}
    >
      {words.changed}
      <span aria-hidden="true"> ›</span>
    </button>
  );
}

/**
 * The Changes tab of one open row (T-6333): its tab entry once the first read answered, its panel,
 * and the mark for a field, which opens the tab (through `open`) filtered to that field.
 */
export function useRowChanges(
  changes: RowChanges | undefined,
  id: string,
  columns: { attr: string; label: string }[],
  open: () => void,
) {
  const state = useEntityChanges(
    changes?.load,
    id,
    columns.map((column) => column.attr).filter((attr) => attr !== ""),
  );
  const [attr, setAttr] = useState<string | null>(null);
  const marked = new Set(
    state?.status === "ready" ? state.changes.map((change) => change.attr) : [],
  );
  const tabs =
    changes && changesShown(state)
      ? [
          {
            id: "changes",
            label: changes.labels?.tab ?? DEFAULT_CHANGES_LABELS.tab,
          },
        ]
      : [];
  const panel =
    changes && state ? (
      <EntityChanges
        state={state}
        attr={attr}
        onAttr={setAttr}
        attrs={columns}
        renderValue={changes.renderValue}
        locale={changes.locale}
        labels={changes.labels}
      />
    ) : null;
  const mark = (field: string | null, label: string) =>
    field !== null && marked.has(field) ? (
      <ChangedMark
        label={label}
        labels={changes?.labels}
        onOpen={() => {
          setAttr(field);
          open();
        }}
      />
    ) : null;
  return { tabs, panel, mark };
}

/**
 * What a grid hands its row dialog for Changes: a reader bound to the open row, stable while the
 * row is (one read per row), and each kept value drawn by the grid's own cell view.
 */
export function useChangesFeed(
  source: EntitySource,
  id: string | undefined,
  locale: string,
  labels: Partial<ChangesLabels> | undefined,
  renderCell: (cell: RichCell) => React.ReactNode,
): RowChanges | undefined {
  const load = useMemo(() => {
    const read = source.changes;
    return read && id
      ? (attrs: string[]) => read.call(source, id, attrs)
      : undefined;
  }, [source, id]);
  if (!load) return undefined;
  const renderValue = (_attr: string, instance: Record<string, unknown>) => {
    const cell = toRichCell(instance, locale);
    return cell ? renderCell(cell) : null;
  };
  return { load, renderValue, locale, labels };
}
