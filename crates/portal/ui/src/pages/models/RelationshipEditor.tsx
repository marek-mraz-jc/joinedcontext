import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  Alert,
  Button,
  Checkbox,
  ConfirmDialog,
  Field,
  Input,
  RadioGroup,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { CARDINALITIES, ON_DELETE_RULES, flagsOf, relationshipsOf, storedEnd } from "./linkml";
import type { Cardinality, LinkmlModel, OnDelete, Relationship } from "./linkml";
import { applyOperations, isName } from "./operations";
import type { Operation } from "./operations";

/**
 * The relationships of one class, and the form that adds one (DM-64…DM-66, DM-73).
 *
 * A relationship is two slots naming each other, so every change here is one operation writing
 * both ends: the form never writes a slot pointing at a class without its inverse. `run` applies
 * the operation to the document and answers the refusal, if any, which is shown beside what
 * caused it as text, never as markup.
 */
export interface RelationshipEditorProps {
  source: string;
  model: LinkmlModel;
  /** The class whose relationships are listed and from which a new one starts. */
  klass: string;
  /** Applies one operation; answers the refusal's reason, or null when it landed. */
  run: (operation: Operation) => string | null;
  /** A class the range picker chose: the form opens on it, with a new key per pick. */
  prefill?: { target: string; key: number };
  /** The add form under the list; the page adds a link through "Add field" and shows it only for a range pick. */
  withAdd?: boolean;
}

/** The small figure beside each cardinality: the source on the left, the target on the right. */
const FIGURES: Record<Cardinality, string> = {
  "one-to-one": "1 ── 1",
  "one-to-many": "1 ──< ∗",
  "many-to-one": "∗ >── 1",
  "many-to-many": "∗ >─< ∗",
};

/** `School` → `school`, and `schools` when the end holds many. */
export function suggestedName(klass: string, many: boolean): string {
  const base = klass.charAt(0).toLowerCase() + klass.slice(1);
  if (!many) return base;
  if (/[^aeiou]y$/.test(base)) return `${base.slice(0, -1)}ies`;
  if (/(s|x|z|ch|sh)$/.test(base)) return `${base}es`;
  return `${base}s`;
}

export function sentence(t: TFunction, relationship: Relationship): string {
  return t(`models.relationships.sentence.${relationship.cardinality}`, {
    source: relationship.source.class,
    target: relationship.target.class,
  });
}

export function RelationshipEditor({ source, model, klass, run, prefill, withAdd = true }: RelationshipEditorProps): JSX.Element {
  const { t } = useTranslation();
  const [refusal, setRefusal] = useState<string | null>(null);
  const [removing, setRemoving] = useState<Relationship | null>(null);
  const found = useMemo(() => relationshipsOf(model), [model]);
  const mine = found.relationships.filter(
    (relationship) => relationship.source.class === klass || relationship.target.class === klass,
  );
  const classSlots = model.classes.find((candidate) => candidate.name === klass)?.slots ?? [];
  // A slot of this class pointing at a class of the model with no inverse: saved before inverses
  // were required, fixed with one button that keeps the data where it is today (DM-73).
  const oneSided = classSlots.flatMap((name) => {
    const slot = model.slots.find((candidate) => candidate.name === name);
    const pointsAtClass = model.classes.some((candidate) => candidate.name === slot?.range);
    return slot?.kind === "Relationship" && slot.inverse === undefined && pointsAtClass
      ? [{ slot: name, target: slot.range ?? "" }]
      : [];
  });

  const apply = (operation: Operation): boolean => {
    const reason = run(operation);
    setRefusal(reason);
    return reason === null;
  };

  return (
    <section aria-labelledby="models-relationships" className="flex flex-col gap-3">
      <h2 id="models-relationships" className="text-sm font-semibold uppercase tracking-wide">
        {t("models.relationships.title")}
      </h2>
      <p className="text-caption text-fg-muted">{t("models.relationships.intro", { owner: klass })}</p>
      {refusal ? (
        <Alert tone="danger" role="alert">
          {t("models.refused", { reason: refusal })}
        </Alert>
      ) : null}
      {oneSided.map(({ slot, target }) => {
        const inverse = suggestedName(klass, true);
        return (
          <div key={slot} className="flex flex-wrap items-center gap-2 rounded border border-border p-2">
            <p role="status" className="text-caption text-warning">
              {t("models.relationships.missingInverse", { name: slot, target })}
            </p>
            <Button size="sm" variant="secondary" onClick={() => apply({ op: "addInverse", name: slot, inverse })}>
              {t("models.relationships.addInverse", { inverse, target })}
            </Button>
          </div>
        );
      })}
      {mine.length === 0 ? (
        <p className="text-body">{t("models.relationships.none", { owner: klass })}</p>
      ) : (
        <Table caption={t("models.relationships.title")}>
          <TableHead>
            <TableHeaderCell>{t("models.relationships.name")}</TableHeaderCell>
            <TableHeaderCell>{t("models.relationships.other")}</TableHeaderCell>
            <TableHeaderCell>{t("models.relationships.cardinality")}</TableHeaderCell>
            <TableHeaderCell>{t("models.relationships.inverse")}</TableHeaderCell>
            <TableHeaderCell>{t("models.relationships.required")}</TableHeaderCell>
            <TableHeaderCell>{t("models.relationships.onDelete")}</TableHeaderCell>
            <TableHeaderCell>{t("models.relationships.actions")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {mine.map((relationship) => {
              // The row reads from this class: its own end first, the other end as the inverse.
              const here = relationship.source.class === klass ? relationship.source : relationship.target;
              const there = here === relationship.source ? relationship.target : relationship.source;
              const required = [relationship.source, relationship.target]
                .filter((end) => end.required)
                .map((end) => `${end.class}.${end.slot}`);
              return (
                <TableRow key={`${here.class}.${here.slot}`}>
                  <TableCell primary>{here.slot}</TableCell>
                  <TableCell>{there.class}</TableCell>
                  <TableCell>
                    <p className="mb-1 text-caption">{sentence(t, relationship)}</p>
                    <Select
                      aria-label={t("models.relationships.cardinalityOf", { name: here.slot })}
                      value={relationship.cardinality}
                      onChange={(event) =>
                        apply({ op: "setCardinality", name: here.slot, cardinality: event.target.value as Cardinality })
                      }
                    >
                      {CARDINALITIES.map((cardinality) => (
                        <option key={cardinality} value={cardinality}>
                          {t(`models.relationships.cardinalityName.${cardinality}`)}
                        </option>
                      ))}
                    </Select>
                  </TableCell>
                  <TableCell>{there.slot}</TableCell>
                  <TableCell>{required.length > 0 ? required.join(", ") : "—"}</TableCell>
                  <TableCell>
                    <Select
                      aria-label={t("models.relationships.onDeleteOf", { name: here.slot })}
                      value={relationship.onDelete}
                      onChange={(event) =>
                        apply({ op: "setOnDelete", name: here.slot, onDelete: event.target.value as OnDelete })
                      }
                    >
                      {ON_DELETE_RULES.map((rule) => (
                        <option key={rule} value={rule}>
                          {t(`models.relationships.onDeleteName.${rule}`)}
                        </option>
                      ))}
                    </Select>
                  </TableCell>
                  <TableCell>
                    <Button
                      size="sm"
                      variant="danger"
                      aria-label={t("models.relationships.removeLabel", { name: here.slot })}
                      onClick={() => setRemoving(relationship)}
                    >
                      {t("models.relationships.remove")}
                    </Button>
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      )}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(open) => {
          if (!open) setRemoving(null);
        }}
        title={removing ? t("models.relationships.confirmTitle", { name: removing.source.slot }) : ""}
        description={
          removing
            ? t("models.relationships.confirmBody", {
                source: removing.source.class,
                sourceSlot: removing.source.slot,
                target: removing.target.class,
                targetSlot: removing.target.slot,
              })
            : undefined
        }
        confirmLabel={t("models.relationships.remove")}
        onConfirm={() => {
          if (removing) apply({ op: "removeRelationship", name: removing.source.slot });
          setRemoving(null);
        }}
      />
      {withAdd ? <AddRelationshipForm
        key={`${klass}:${prefill?.key ?? 0}`}
        source={source}
        model={model}
        klass={klass}
        initialTarget={prefill?.target}
        run={run}
      /> : null}
    </section>
  );
}

export interface AddRelationshipFormProps {
  source: string;
  model: LinkmlModel;
  klass: string;
  initialTarget?: string;
  run: (operation: Operation) => string | null;
  /** Inside a dialog: no frame or heading of its own, the dialog gives both. */
  bare?: boolean;
}

export function AddRelationshipForm({ source, model, klass, initialTarget, run, bare = false }: AddRelationshipFormProps): JSX.Element {
  const { t } = useTranslation();
  const [cardinality, setCardinality] = useState<Cardinality>("one-to-many");
  const [target, setTarget] = useState(initialTarget ?? "");
  // Null follows the suggestion; typing takes the field over.
  const [name, setName] = useState<string | null>(null);
  const [inverse, setInverse] = useState<string | null>(null);
  const [required, setRequired] = useState(false);
  const [inverseRequired, setInverseRequired] = useState(false);
  const [onDelete, setOnDelete] = useState<OnDelete>("restrict");

  const flags = flagsOf(cardinality);
  const stored = storedEnd(cardinality);
  const slotName = name ?? (target ? suggestedName(target, flags.source) : "");
  const inverseName = inverse ?? (target ? suggestedName(klass, flags.target) : "");
  const taken = (candidate: string) => model.slots.some((slot) => slot.name === candidate);

  const nameError = !slotName
    ? t("models.relationships.add.nameEmpty")
    : !isName(slotName)
      ? t("models.relationships.add.nameInvalid")
      : taken(slotName)
        ? t("models.relationships.add.nameTaken", { name: slotName })
        : undefined;
  const inverseError = !inverseName
    ? t("models.relationships.add.nameEmpty")
    : !isName(inverseName)
      ? t("models.relationships.add.nameInvalid")
      : taken(inverseName)
        ? t("models.relationships.add.nameTaken", { name: inverseName })
        : inverseName === slotName
          ? t("models.relationships.add.inverseSame")
          : undefined;

  const operation: Operation = {
    op: "addRelationship",
    from: klass,
    to: target,
    name: slotName,
    inverse: inverseName,
    cardinality,
    // Only the stored end can be required (DM-65); the computed end's box is disabled.
    required: stored === "source" && required,
    inverseRequired: stored === "target" && inverseRequired,
    onDelete,
  };
  const ready = target !== "" && nameError === undefined && inverseError === undefined;
  // The same operation on a copy: whatever the editor would refuse keeps Add disabled, with why.
  const refusal = ready ? (applyOperations(source, [operation]).refused[0]?.reason ?? null) : null;

  const submit = () => {
    if (run(operation) === null) {
      setTarget("");
      setName(null);
      setInverse(null);
      setRequired(false);
      setInverseRequired(false);
    }
  };

  // Before a class is picked the labels name it in words, not by the Target class label (UI judge, T-6344).
  const targetLabel = target || t("models.relationships.add.targetPending");
  const requiredHint = (end: "source" | "target", owner: string) =>
    stored === end
      ? end === "target" && !target
        ? t("models.relationships.add.requiredStoredPending")
        : t("models.relationships.add.requiredStored", { owner })
      : t("models.relationships.add.requiredComputed");

  return (
    <form
      aria-labelledby="models-relationship-add"
      className={bare ? "flex flex-col gap-4" : "flex flex-col gap-3 rounded border border-border p-3"}
      onSubmit={(event) => {
        event.preventDefault();
        if (ready && refusal === null) submit();
      }}
    >
      <h3 id="models-relationship-add" className={bare ? "sr-only" : "text-sm font-semibold"}>
        {t("models.relationships.add.title", { owner: klass })}
      </h3>
      <RadioGroup
        name="relationship-cardinality"
        legend={t("models.relationships.cardinality")}
        description={t("models.relationships.add.cardinalityHint", { owner: klass })}
        value={cardinality}
        layout="row"
        className="[&>div]:grid [&>div]:grid-cols-2 [&>div]:gap-x-6 [&>div]:gap-y-2"
        onChange={(value) => {
          setCardinality(value);
          if (storedEnd(value) === "source") setInverseRequired(false);
          else setRequired(false);
        }}
        options={CARDINALITIES.map((value) => ({
          value,
          label: (
            <span className="flex items-center gap-2">
              <span aria-hidden="true" className="font-mono text-caption">
                {FIGURES[value]}
              </span>
              {t(`models.relationships.cardinalityName.${value}`)}
            </span>
          ),
        }))}
      />
      <div className="grid items-start gap-x-4 gap-y-3 sm:grid-cols-2">
        <Field id="relationship-target" label={t("models.relationships.add.target")} help={t("models.relationships.add.targetHint")}>
          <Select id="relationship-target" value={target} onChange={(event) => setTarget(event.target.value)}>
            <option value="">{t("models.relationships.add.targetNone")}</option>
            {model.classes.map((candidate) => (
              <option key={candidate.name} value={candidate.name}>
                {candidate.name}
              </option>
            ))}
          </Select>
        </Field>
        <Field
          id="relationship-name"
          label={t("models.relationships.add.name", { owner: klass })}
          help={t("models.relationships.add.nameHint", { owner: klass })}
          errors={target && nameError ? [nameError] : undefined}
        >
          <Input
            id="relationship-name"
            value={slotName}
            onChange={(event) => setName(event.target.value.trim())}
          />
        </Field>
        <Field
          id="relationship-inverse"
          required
          label={t("models.relationships.add.inverse", { target: targetLabel })}
          help={t("models.relationships.add.inverseHint")}
          errors={target && inverseError ? [inverseError] : undefined}
        >
          <Input id="relationship-inverse" value={inverseName} onChange={(event) => setInverse(event.target.value.trim())} />
        </Field>
        <Field
          id="relationship-on-delete"
          label={t("models.relationships.onDelete")}
          help={t(`models.relationships.onDeleteHint.${onDelete}`)}
        >
          <Select
            id="relationship-on-delete"
            value={onDelete}
            onChange={(event) => setOnDelete(event.target.value as OnDelete)}
          >
            {ON_DELETE_RULES.map((rule) => (
              <option key={rule} value={rule}>
                {t(`models.relationships.onDeleteName.${rule}`)}
              </option>
            ))}
          </Select>
        </Field>
      </div>
      <div className="grid items-start gap-x-4 gap-y-3 sm:grid-cols-2">
        <Field id="relationship-required" hideLabel help={requiredHint("source", klass)}>
          <Checkbox
            id="relationship-required"
            label={t("models.relationships.add.requiredOn", { owner: klass })}
            checked={required}
            disabled={stored !== "source"}
            disabledReason={t("models.relationships.add.requiredComputed")}
            onChange={(event) => setRequired(event.target.checked)}
          />
        </Field>
        <Field id="relationship-inverse-required" hideLabel help={requiredHint("target", targetLabel)}>
          <Checkbox
            id="relationship-inverse-required"
            label={t("models.relationships.add.requiredOn", { owner: targetLabel })}
            checked={inverseRequired}
            disabled={stored !== "target"}
            disabledReason={t("models.relationships.add.requiredComputed")}
            onChange={(event) => setInverseRequired(event.target.checked)}
          />
        </Field>
      </div>
      {refusal ? (
        <Alert tone="danger" role="alert">
          {t("models.refused", { reason: refusal })}
        </Alert>
      ) : null}
      {/* A sighted person reads why Add relationship waits; the button's reason is for screen readers. */}
      {!ready && refusal === null ? (
        <p className={`text-caption text-fg-muted${bare ? " self-end" : ""}`}>{t("models.relationships.add.pickFirst")}</p>
      ) : null}
      <Button type="submit" variant="primary" className={bare ? "w-fit self-end" : "w-fit"} disabled={!ready || refusal !== null}
        disabledReason={refusal ?? t("models.relationships.add.pickFirst")}
      >
        {t("models.relationships.add.submit")}
      </Button>
    </form>
  );
}
