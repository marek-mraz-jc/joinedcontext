/**
 * Editing a data model on its drawing (T-3589): a relationship added by dragging from one class
 * to another (or from a class's "+"), changed or removed from its line, a class or a field
 * added in place, and the last edits undone before the model is saved.
 *
 * Every edit is an `Operation` of `operations.ts`, applied to the editor's one source string,
 * exactly as the structure view's forms apply theirs, so the drawing writes the same LinkML the
 * forms do. An edit the breaking-change detector calls breaking (a removed or renamed field a
 * stored entity may use) is not applied until the person confirms it; the page's Save then
 * proposes the whole edit as one Change with its diff and its version bump, as before.
 */
import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Alert, Button, ConfirmDialog, Dialog, Field, Input, Select } from "../../components/ui";
import { classifyChanges } from "./breaking_detector";
import { CARDINALITIES, RANGES, newTypeParent, parseModel } from "./linkml";
import type { Cardinality, GraphEdge } from "./linkml";
import { applyOperations, isName } from "./operations";
import type { Operation } from "./operations";
import { AddRelationshipForm } from "./RelationshipEditor";

export interface DiagramEditing {
  /** The new-relationship form from `from`, on `to` when a drag ended on it. */
  addRelationship: (from: string, to?: string) => void;
  /** The form that changes or removes the relationship a line draws. */
  editRelationship: (edge: GraphEdge) => void;
  addClass: () => void;
  addField: (klass: string) => void;
  undo: () => void;
  /** Whether the last edit made here is still the document as it stands. */
  canUndo: boolean;
  /** The dialogs the edits open; rendered once beside the drawing. */
  dialogs: JSX.Element;
}

/** The edits of the drawing over `source`; `undefined` when there is no `onChange` to write to. */
export function useDiagramEditing(source: string, onChange?: (source: string) => void): DiagramEditing | undefined {
  const { t } = useTranslation();
  const model = useMemo(() => parseModel(source), [source]);
  const [history, setHistory] = useState<{ before: string; after: string }[]>([]);
  const [adding, setAdding] = useState<{ from: string; to?: string; key: number }>();
  const [editing, setEditing] = useState<GraphEdge>();
  const [classOpen, setClassOpen] = useState(false);
  const [fieldOf, setFieldOf] = useState<string>();
  const [pending, setPending] = useState<{ after: string; reasons: string[] }>();

  if (!onChange) return undefined;

  const commit = (after: string) => {
    setHistory((past) => [...past, { before: source, after }]);
    onChange(after);
  };
  /**
   * What every dialog calls: the refusal's reason, or null when the edit landed or waits for the
   * person to confirm that it breaks the model.
   */
  const run = (operations: Operation[]): string | null => {
    const applied = applyOperations(source, operations);
    const refused = applied.refused[0];
    if (refused) return refused.reason;
    const breaking = classifyChanges(model, parseModel(applied.source)).filter((change) => change.severity === "breaking");
    if (breaking.length > 0) {
      setPending({ after: applied.source, reasons: breaking.map((change) => `${change.subject}: ${change.reason}`) });
    } else {
      commit(applied.source);
    }
    return null;
  };
  const last = history[history.length - 1];
  const canUndo = last !== undefined && last.after === source;

  const dialogs = (
    <>
      <Dialog
        open={adding !== undefined}
        onOpenChange={(open) => {
          if (!open) setAdding(undefined);
        }}
        title={t("models.graph.edit.addRelationshipTitle", { name: adding?.from ?? "" })}
        closeLabel={t("app.close")}
        size="lg"
      >
        {adding ? (
          <AddRelationshipForm
            key={adding.key}
            source={source}
            model={model}
            klass={adding.from}
            initialTarget={adding.to}
            run={(operation) => {
              const reason = run([operation]);
              if (reason === null) setAdding(undefined);
              return reason;
            }}
          />
        ) : null}
      </Dialog>
      {editing ? (
        <EditRelationship
          edge={editing}
          taken={(name) => model.slots.some((slot) => slot.name === name)}
          run={run}
          onClose={() => setEditing(undefined)}
        />
      ) : null}
      {classOpen ? (
        <NameDialog
          title={t("models.graph.edit.addClassTitle")}
          label={t("models.graph.edit.className")}
          taken={(name) => model.classes.some((klass) => klass.name === name)}
          run={(name) => run([{ op: "addClass", name, ...newTypeParent(model) }])}
          onClose={() => setClassOpen(false)}
        />
      ) : null}
      {fieldOf !== undefined ? (
        <NameDialog
          title={t("models.graph.edit.addFieldTitle", { name: fieldOf })}
          label={t("models.graph.edit.fieldName")}
          taken={(name) => model.slots.some((slot) => slot.name === name)}
          ranges={[...RANGES, ...model.enums.map((entry) => entry.name)]}
          run={(name, range) => run([{ op: "addSlot", name, class: fieldOf, range }])}
          onClose={() => setFieldOf(undefined)}
        />
      ) : null}
      <ConfirmDialog
        open={pending !== undefined}
        onOpenChange={(open) => {
          if (!open) setPending(undefined);
        }}
        tone="danger"
        title={t("models.graph.edit.breakingTitle")}
        description={t("models.graph.edit.breakingBody")}
        confirmLabel={t("models.graph.edit.breakingConfirm")}
        onConfirm={() => {
          if (pending) commit(pending.after);
          setPending(undefined);
        }}
      >
        <ul className="list-disc pl-5 text-body">
          {pending?.reasons.map((reason) => <li key={reason}>{reason}</li>)}
        </ul>
      </ConfirmDialog>
    </>
  );

  return {
    addRelationship: (from, to) => setAdding({ from, to, key: Date.now() }),
    editRelationship: setEditing,
    addClass: () => setClassOpen(true),
    addField: setFieldOf,
    undo: () => {
      if (!canUndo) return;
      setHistory((past) => past.slice(0, -1));
      onChange(last.before);
    },
    canUndo,
    dialogs,
  };
}

/** Why `name` cannot be a new name, or undefined when it can. */
function nameError(t: (key: string, values?: Record<string, string>) => string, name: string, taken: boolean): string | undefined {
  if (!name) return t("models.relationships.add.nameEmpty");
  if (!isName(name)) return t("models.relationships.add.nameInvalid");
  return taken ? t("models.relationships.add.nameTaken", { name }) : undefined;
}

/** A name, and for a field its range: one dialog for "add a class" and "add a field". */
function NameDialog({
  title,
  label,
  taken,
  ranges,
  run,
  onClose,
}: {
  title: string;
  label: string;
  taken: (name: string) => boolean;
  /** The ranges a new field may take; absent for a class. */
  ranges?: readonly string[];
  run: (name: string, range: string) => string | null;
  onClose: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [range, setRange] = useState(ranges?.[0] ?? "");
  const [refusal, setRefusal] = useState<string | null>(null);
  const error = nameError(t, name, taken(name));
  const submit = () => {
    if (error) return;
    const reason = run(name, range);
    setRefusal(reason);
    if (reason === null) onClose();
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={title}
      closeLabel={t("app.close")}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("app.cancel")}
          </Button>
          <Button variant="primary" disabled={error !== undefined} disabledReason={error} onClick={submit}>
            {t("models.graph.edit.add")}
          </Button>
        </>
      }
    >
      <form
        className="flex flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <Field id="diagram-new-name" label={label} errors={name && error ? [error] : undefined}>
          <Input id="diagram-new-name" value={name} onChange={(event) => setName(event.target.value.trim())} />
        </Field>
        {ranges ? (
          <Field id="diagram-new-range" label={t("models.graph.edit.fieldRange")}>
            <Select id="diagram-new-range" value={range} onChange={(event) => setRange(event.target.value)}>
              {ranges.map((option) => (
                <option key={option} value={option}>
                  {option}
                </option>
              ))}
            </Select>
          </Field>
        ) : null}
        {refusal ? (
          <Alert tone="danger" role="alert">
            {t("models.refused", { reason: refusal })}
          </Alert>
        ) : null}
      </form>
    </Dialog>
  );
}

/**
 * One relationship from its line: rename either end, change how many, or remove it. A rename
 * renames the slot wherever the model lists it (`renameSlot`), as the structure view does.
 */
function EditRelationship({
  edge,
  taken,
  run,
  onClose,
}: {
  edge: GraphEdge;
  taken: (name: string) => boolean;
  run: (operations: Operation[]) => string | null;
  onClose: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const was = { name: edge.label ?? "", inverse: edge.inverse ?? "", cardinality: edge.cardinality ?? "one-to-one" };
  const [name, setName] = useState(was.name);
  const [inverse, setInverse] = useState(was.inverse);
  const [cardinality, setCardinality] = useState<Cardinality>(was.cardinality);
  const [refusal, setRefusal] = useState<string | null>(null);
  const nameProblem = name === was.name ? undefined : nameError(t, name, taken(name));
  const inverseProblem =
    inverse === was.inverse
      ? undefined
      : (nameError(t, inverse, taken(inverse)) ?? (inverse === name ? t("models.relationships.add.inverseSame") : undefined));
  const operations: Operation[] = [
    ...(name !== was.name ? [{ op: "renameSlot" as const, name: was.name, to: name }] : []),
    ...(inverse !== was.inverse ? [{ op: "renameSlot" as const, name: was.inverse, to: inverse }] : []),
    ...(cardinality !== was.cardinality ? [{ op: "setCardinality" as const, name, cardinality }] : []),
  ];
  const finish = (reason: string | null) => {
    setRefusal(reason);
    if (reason === null) onClose();
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={t("models.graph.edit.relationshipTitle", { name: was.name, inverse: was.inverse })}
      description={t(`models.relationships.sentence.${was.cardinality}`, { source: edge.from, target: edge.to })}
      closeLabel={t("app.close")}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("app.cancel")}
          </Button>
          <Button variant="danger" onClick={() => finish(run([{ op: "removeRelationship", name: was.name }]))}>
            {t("models.relationships.remove")}
          </Button>
          <Button
            variant="primary"
            disabled={operations.length === 0 || nameProblem !== undefined || inverseProblem !== undefined}
            disabledReason={nameProblem ?? inverseProblem ?? t("models.graph.edit.nothingToApply")}
            onClick={() => finish(run(operations))}
          >
            {t("models.graph.edit.apply")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <Field id="diagram-relationship-name" label={t("models.relationships.add.name", { owner: edge.from })} errors={nameProblem ? [nameProblem] : undefined}>
          <Input id="diagram-relationship-name" value={name} onChange={(event) => setName(event.target.value.trim())} />
        </Field>
        <Field id="diagram-relationship-inverse" label={t("models.relationships.add.inverse", { target: edge.to })} errors={inverseProblem ? [inverseProblem] : undefined}>
          <Input id="diagram-relationship-inverse" value={inverse} onChange={(event) => setInverse(event.target.value.trim())} />
        </Field>
        <Field id="diagram-relationship-cardinality" label={t("models.relationships.cardinality")}>
          <Select
            id="diagram-relationship-cardinality"
            value={cardinality}
            onChange={(event) => setCardinality(event.target.value as Cardinality)}
          >
            {CARDINALITIES.map((option) => (
              <option key={option} value={option}>
                {t(`models.relationships.cardinalityName.${option}`)}
              </option>
            ))}
          </Select>
        </Field>
        {refusal ? (
          <Alert tone="danger" role="alert">
            {t("models.refused", { reason: refusal })}
          </Alert>
        ) : null}
      </div>
    </Dialog>
  );
}
