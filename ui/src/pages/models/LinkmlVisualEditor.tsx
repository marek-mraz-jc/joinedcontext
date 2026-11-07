import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import {
  Alert,
  Badge,
  Button,
  Checkbox,
  Dialog,
  Field,
  Icon,
  Input,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Term,
  RadioGroup,
  fieldIds,
} from "../../components/ui";
import {
  DEFAULT_KIND,
  NGSI_LD_KINDS,
  RANGES,
  newTypeParent,
  parseModel,
  reservedNamespace,
  slotAffordance,
  unitCode,
  UPSTREAM_ANNOTATION,
  slotOrigins,
  valueClasses,
} from "./linkml";
import type { Diagnostic, LinkmlSlot, NgsiLdKind } from "./linkml";
import { applyOperations } from "./operations";
import { UnitPicker } from "../../components/pickers/UnitPicker";
import { unitSymbol } from "../../units";
import { dropsUnit, rangeForUnit, unitNotice } from "./unitRule";
import type { UnitNotice } from "./unitRule";
import type { Operation, SlotField } from "./operations";
import { EnumValues } from "./EnumValues";
import { NameField } from "./NameField";
import { AddRelationshipForm, RelationshipEditor } from "./RelationshipEditor";
import { fieldWord, FIELD_RANGE, useFieldWords } from "../../components/entities/fieldTypes";

/** A basic range as the Add field dialog names it, or the range itself where it has no word. */
const RANGE_TYPE = new Map(
  Object.entries(FIELD_RANGE)
    .filter(([type]) => type !== "email" && type !== "file" && type !== "formula")
    .map(([type, { range }]) => [range, type]),
);

/**
 * The structured view of the model: classes, slots, enums (DM-13).
 *
 * Every edit is one `Operation` applied to the YAML document and handed back as text, so this
 * view and the source view are two renderings of one document rather than two copies of it,
 * and an assistant editing through `applyOperations` does exactly what a click here does
 * (DM-31). Nothing is mutated in place, and a source that does not parse leaves the view empty
 * rather than showing a stale tree.
 */
export interface LinkmlVisualEditorProps {
  source: string;
  onChange: (source: string) => void;
  diagnostics?: Diagnostic[];
  /** The organisation's configured locales, for the language maps of DM-15. */
  locales?: string[];
  /** The class to open first, when the diagram named one. */
  initialClass?: string;
  /** Where "Import a Smart Data Model" goes, when the page around the editor offers the import. */
  /** Opens the Portal's import of types from a Smart Data Model or a spreadsheet. */
  onImport?: (from: "sdm" | "file") => void;
}

/** The kinds a new field starts as, in the order a person reads them. */
const ADD_KINDS = ["Property", "Relationship", "GeoProperty", "LanguageProperty"] as const;

export function LinkmlVisualEditor({
  source,
  onChange,
  diagnostics = [],
  locales = [],
  onImport,
  initialClass,
}: LinkmlVisualEditorProps): JSX.Element {
  const { t } = useTranslation();
  const word = useFieldWords();
  const model = useMemo(() => parseModel(source), [source]);

  const [selectedClass, setSelectedClass] = useState<string | null>(initialClass ?? null);
  const [selectedSlot, setSelectedSlot] = useState<string | null>(null);
  // An enumeration opens its values table in place of the type (T-6162).
  const [selectedEnum, setSelectedEnum] = useState<string | null>(null);
  const [typeSearch, setTypeSearch] = useState("");
  const [newClass, setNewClass] = useState("");
  const [addingClass, setAddingClass] = useState(false);
  const [newSlot, setNewSlot] = useState("");
  const [addingSlot, setAddingSlot] = useState(false);
  const [addKind, setAddKind] = useState<(typeof ADD_KINDS)[number]>("Property");
  const [newEnum, setNewEnum] = useState("");
  // One box per enum: a shared one filled every enum's box with what was typed in one (T-2706).
  const [iriError, setIriError] = useState<string | null>(null);
  // The IRI field keeps what was typed even while it is refused: a controlled input that
  // drops the keystroke would fight the person editing it.
  const [iriDraft, setIriDraft] = useState<string | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  // The class the range picker chose opens Add a relationship on it; a new key per pick.
  const [prefill, setPrefill] = useState<{ target: string; key: number } | undefined>(undefined);

  const activeClass =
    model.classes.find((klass) => klass.name === selectedClass) ?? model.classes[0];
  const activeSlot = model.slots.find((slot) => slot.name === selectedSlot);
  const enumOpen = model.enums.find((entry) => entry.name === selectedEnum);
  const origins = activeClass ? slotOrigins(model, activeClass) : new Map<string, string>();
  // A value structure's parts are inline `attributes` (Smart Data Models): listed, read-only.
  const inlineParts = new Map((activeClass?.attributes ?? []).filter((one) => !origins.has(one.name)).map((one) => [one.name, one]));
  for (const name of inlineParts.keys()) origins.set(name, activeClass?.name ?? "");
  // Entity types in the rail; the shapes of structured values (a JsonProperty's range) apart, under them.
  const structures = valueClasses(model);
  // A field inherited from a parent is shared by every type below it: it opens read-only here.
  const activeSlotOwner = activeSlot ? origins.get(activeSlot.name) : undefined;
  const activeSlotInheritedFrom =
    activeSlotOwner && activeSlotOwner !== activeClass?.name ? activeSlotOwner : undefined;

  /** One operation, the same way an assistant would send it; a refusal is shown, not swallowed. */
  const run = (operation: Operation): boolean => {
    const reason = runQuietly(operation);
    setRefusal(reason);
    return reason === null;
  };

  /**
   * A unit and the type of value move together (T-6101): the second change is made with the
   * first, said in words, and one Undo puts the document back as it was before both. Undo
   * stays only while the document is still the one the pick made: a later edit would be lost.
   */
  const [unitUndo, setUnitUndo] = useState<
    { before: string; after: string; notice: UnitNotice; slot: string } | null
  >(null);
  const runPaired = (slot: string, operations: Operation[], notice: UnitNotice | undefined) => {
    const before = source;
    const applied = applyOperations(source, operations);
    setRefusal(applied.refused[0]?.reason ?? null);
    if (applied.refused.length > 0) return;
    onChange(applied.source);
    setUnitUndo(notice ? { before, after: applied.source, notice, slot } : null);
  };
  const pickUnit = (slot: LinkmlSlot, code: string) => {
    const range = rangeForUnit(slot.range, code);
    runPaired(
      slot.name,
      [
        ...(range ? [{ op: "setSlot", name: slot.name, field: "range", value: range } as Operation] : []),
        { op: "setSlot", name: slot.name, field: "unit", value: code },
      ],
      unitNotice(slot.range, code),
    );
  };
  const pickRange = (slot: LinkmlSlot, range: string) => {
    const unit = unitCode(slot.unit);
    const drop = dropsUnit(range, unit);
    runPaired(
      slot.name,
      [
        { op: "setSlot", name: slot.name, field: "range", value: range },
        ...(drop ? [{ op: "setSlot", name: slot.name, field: "unit", value: "" } as Operation] : []),
      ],
      drop ? "droppedUnit" : undefined,
    );
  };

  /** The same, answering the refusal to the section that shows it beside its own fields. */
  const runQuietly = (operation: Operation): string | null => {
    const applied = applyOperations(source, [operation]);
    if (applied.refused.length === 0) {
      onChange(applied.source);
    }
    return applied.refused[0]?.reason ?? null;
  };

  /** Opening another slot starts its IRI field from the document, not from the last draft. */
  const openSlot = (name: string) => {
    setSelectedSlot(name);
    setIriDraft(null);
    setIriError(null);
  };

  const addClass = () => {
    const name = newClass.trim();
    if (!name) {
      return;
    }
    if (run({ op: "addClass", name, ...newTypeParent(model) })) {
      setSelectedClass(name);
      setNewClass("");
    }
  };

  const setSlotField = (name: string, field: SlotField, value: unknown) =>
    run({ op: "setSlot", name, field, value });

  /**
   * A slot IRI is written only when it is the organisation's to mint (DM-16). A term under a
   * reserved namespace is refused here rather than flagged later, because the model would be
   * unpublishable and the IRI is exactly the thing consumers resolve.
   */
  const setSlotIri = (name: string, value: string) => {
    setIriDraft(value);
    const trimmed = value.trim();
    const slot = model.slots.find((candidate) => candidate.name === name);
    const reserved = trimmed ? reservedNamespace(trimmed, model.prefixes) : undefined;
    if (reserved && !slot?.upstream) {
      setIriError(t("models.iriReserved", { iri: trimmed, namespace: reserved }));
      return;
    }
    setIriError(null);
    setSlotField(name, "slot_uri", trimmed);
  };

  const setTitle = (target: "class" | "slot", name: string, locale: string, value: string) =>
    run({ op: "setTitle", target, name, locale, value });

  const addEnum = () => {
    const name = newEnum.trim();
    if (!name) {
      return;
    }
    if (run({ op: "addEnum", name })) {
      setNewEnum("");
      setSelectedEnum(name);
    }
  };

  const messagesFor = (path: string) =>
    diagnostics.filter((diagnostic) => diagnostic.path === path);

  return (
    // `minmax(0, 1fr)`, not `auto`: an auto column grew to the attribute table's width and the
    // page scrolled sideways at 375 px; the table scrolls in its own frame (T-4165, audit T-4159).
    <div className="grid grid-cols-1 gap-6 lg:grid-cols-[16rem_minmax(0,1fr)]">
      {refusal ? (
        <Alert tone="danger" role="alert" className="lg:col-span-2">
          {t("models.refused", { reason: refusal })}
        </Alert>
      ) : null}
      {/* On a phone the rail goes under the type it opens; a picker above takes its place (UI judge, T-6106). */}
      {/* A model with no type yet keeps the rail first: its Add type is the only next step (T-6254). */}
      <div className={`${model.classes.length > 0 ? "order-last" : ""} flex flex-col gap-6 lg:order-none`}>
        <section aria-labelledby="models-classes">
          <h2 id="models-classes" className="mb-2 text-sm font-semibold uppercase tracking-wide">
            {t("models.classes")}
          </h2>
          {model.classes.length > 5 ? (
            <Input
              type="search"
              className="mb-2"
              aria-label={t("models.findType")}
              placeholder={t("models.findType")}
              value={typeSearch}
              onChange={(event) => setTypeSearch(event.target.value)}
            />
          ) : null}
          <ul className="flex flex-col gap-1">
            {model.classes
              .filter((klass) => !structures.has(klass.name))
              .filter((klass) => klass.name.toLowerCase().includes(typeSearch.trim().toLowerCase()))
              .map((klass) => (
              <li key={klass.name}>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    setSelectedClass(klass.name);
                    setSelectedEnum(null);
                  }}
                  aria-current={!enumOpen && klass.name === activeClass?.name ? "true" : undefined}
                  className={
                    !enumOpen && klass.name === activeClass?.name
                      ? "w-full justify-start bg-primary-soft font-semibold text-primary-soft-fg border-l-4 border-primary"
                      : "w-full justify-start"
                  }
                >
                  {klass.name}
                </Button>
              </li>
            ))}
          </ul>
          {structures.size > 0 ? (
            <details className="mt-2 text-body">
              <summary className="cursor-pointer text-caption text-fg-muted">
                {t("models.structures", { count: structures.size })}
              </summary>
              <p className="mt-1 text-caption text-fg-muted">{t("models.structuresHint")}</p>
              <ul className="mt-1 flex flex-col gap-1">
                {[...structures].map((name) => (
                  <li key={name}>
                    <Button
                      variant="ghost"
                      size="sm"
                      className={name === activeClass?.name ? "w-full justify-start bg-primary-soft font-semibold" : "w-full justify-start"}
                      aria-current={!enumOpen && name === activeClass?.name ? "true" : undefined}
                      onClick={() => {
                        setSelectedClass(name);
                        setSelectedEnum(null);
                      }}
                    >
                      {name}
                    </Button>
                  </li>
                ))}
              </ul>
            </details>
          ) : null}
          {/* One "Add type" (owner 2026-11-01): its dialog offers a Smart Data Model or a name, no import
              button beside the list. */}
          <Button size="sm" className="mt-2 w-full" icon={<Icon name="plus" className="size-4" />} onClick={() => setAddingClass(true)}>
            {t("models.addClass")}
          </Button>
          <Dialog open={addingClass} onOpenChange={setAddingClass} title={t("models.addClass")} closeLabel={t("app.close")} size="md">
            <div className="flex flex-col gap-4">
              {onImport ? (
                <div className="grid gap-2 sm:grid-cols-2">
                  {(["sdm", "file"] as const).map((from) => (
                    <Button
                      key={from}
                      variant="secondary"
                      className="!h-auto flex-col !items-start gap-1 py-3 text-left whitespace-normal"
                      onClick={() => {
                        setAddingClass(false);
                        onImport(from);
                      }}
                    >
                      <span className="font-semibold">{t(`models.addFrom.${from}`)}</span>
                      <span className="text-caption text-fg-muted">{t(`models.addFrom.${from}Hint`)}</span>
                    </Button>
                  ))}
                </div>
              ) : null}
              <form
                className="flex flex-col gap-2 border-t border-border pt-4"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (newClass.trim()) {
                    addClass();
                    setAddingClass(false);
                  }
                }}
              >
                <Field id="new-class-name" label={t("models.newClass")} help={t("models.addClassNameHint")}>
                  <div className="flex gap-2">
                    <Input id="new-class-name" aria-describedby={fieldIds("new-class-name").help} value={newClass} onChange={(event) => setNewClass(event.target.value)} />
                    <Button type="submit" variant="primary" disabled={!newClass.trim()} disabledReason={t("models.nameFirst")}>
                      {t("models.add")}
                    </Button>
                  </div>
                </Field>
              </form>
            </div>
          </Dialog>
        </section>

        <section aria-labelledby="models-enums">
          <h2 id="models-enums" className="mb-2 text-sm font-semibold uppercase tracking-wide">
            {t("models.enums")}
          </h2>
          {model.enums.length === 0 ? (
            <p className="text-caption text-fg-muted">{t("models.enumValues.none")}</p>
          ) : (
            <ul className="flex flex-col gap-1">
              {model.enums.map((entry) => (
                <li key={entry.name}>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setSelectedEnum(entry.name)}
                    aria-current={entry.name === enumOpen?.name ? "true" : undefined}
                    className={
                      entry.name === enumOpen?.name
                        ? "w-full justify-start bg-primary-soft font-semibold text-primary-soft-fg border-l-4 border-primary"
                        : "w-full justify-start"
                    }
                  >
                    {entry.name}
                    <span className="ms-auto text-caption font-normal text-fg-muted">{entry.permissible_values.length}</span>
                  </Button>
                </li>
              ))}
            </ul>
          )}
          <div className="mt-2 flex gap-1">
            <Input
              aria-label={t("models.newEnum")}
              placeholder={t("models.newEnum")}
              value={newEnum}
              onChange={(event) => setNewEnum(event.target.value)}
            />
            <Button
              size="sm"
              aria-label={t("models.addEnum")}
              onClick={addEnum}
            >
              {t("models.add")}
            </Button>
          </div>
        </section>
      </div>

      <div className="flex flex-col gap-6">
        {enumOpen ? (
          <EnumValues key={enumOpen.name} model={model} entry={enumOpen} locales={locales} run={run} onRenamed={setSelectedEnum} />
        ) : (
        <>
        {model.classes.length > 1 ? (
          <Field
            id="class-phone-picker"
            label={t("models.typePicker")}
            description={t("models.typePickerHint")}
            className="lg:hidden"
          >
            <Select
              id="class-phone-picker"
              value={activeClass?.name ?? ""}
              onChange={(event) => setSelectedClass(event.target.value)}
            >
              {model.classes.map((klass) => (
                <option key={klass.name} value={klass.name}>
                  {klass.name}
                </option>
              ))}
            </Select>
          </Field>
        ) : null}
        {activeClass ? (
          <section aria-labelledby="models-class-detail" className="flex flex-col gap-3">
            <h2 id="models-class-detail" className="text-base font-semibold">
              {activeClass.name}
            </h2>
            <NameField
              id="class-name"
              label={t("models.className")}
              help={t("models.typeName.hint")}
              name={activeClass.name}
              onRename={(to) => {
                const done = run({ op: "renameClass", name: activeClass.name, to });
                if (done) setSelectedClass(to);
                return done;
              }}
            />
            <div className="grid gap-3 sm:grid-cols-2">
              {/* T-1088, DM-13: the hierarchy the model declares, editable where it is read.
                  A parent can only be another class of this model, so it is a choice, not a
                  free text a typo turns into a dangling reference. */}
              <Field id="class-parent" label={t("models.classParent")}>
                <Select
                  id="class-parent"
                  value={activeClass.is_a ?? ""}
                  onChange={(event) =>
                    run({
                      op: "setClass",
                      name: activeClass.name,
                      field: "is_a",
                      value: event.target.value,
                    })
                  }
                >
                  <option value="">{t("models.classParentNone")}</option>
                  {model.classes
                    .filter((other) => other.name !== activeClass.name)
                    .map((other) => (
                      <option key={other.name} value={other.name}>
                        {other.name}
                      </option>
                    ))}
                </Select>
              </Field>
              <Field id="class-description" label={t("models.description")}>
                <Input
                  id="class-description"
                  value={activeClass.description ?? ""}
                  onChange={(event) =>
                    run({
                      op: "setClass",
                      name: activeClass.name,
                      field: "description",
                      value: event.target.value,
                    })
                  }
                />
              </Field>
            </div>
            {/* What only a modeller needs (T-4829): the IRI is minted from the organisation's
                prefix when the type is added, so a steward never has to type one. */}
            <details className="rounded border border-border p-2">
              <summary className="cursor-pointer text-body font-medium">{t("models.advanced")}</summary>
              <div className="mt-2 grid gap-3 sm:grid-cols-2">
                  <Field id="class-uri" label={t("models.classUri")}>
                    <Input
                      id="class-uri"
                      value={activeClass.class_uri ?? ""}
                      onChange={(event) =>
                        run({
                          op: "setClass",
                          name: activeClass.name,
                          field: "class_uri",
                          value: event.target.value,
                        })
                      }
                    />
                  </Field>
              <Field id="class-mixins" label={t("models.classMixins")}>
                <Input
                  id="class-mixins"
                  value={(activeClass.mixins ?? []).join(", ")}
                  onChange={(event) =>
                    run({
                      op: "setClassMixins",
                      name: activeClass.name,
                      mixins: event.target.value.split(","),
                    })
                  }
                />
              </Field>
              </div>
            </details>
            {locales.length > 0 ? (
              <TitleRow
                locales={locales}
                title={activeClass.title}
                onChange={(locale, value) => setTitle("class", activeClass.name, locale, value)}
              />
            ) : null}
            {messagesFor(`classes.${activeClass.name}`).map((diagnostic) => (
              <p
                key={diagnostic.message}
                role="status"
                className="text-caption text-warning"
              >
                {diagnostic.message}
              </p>
            ))}
          </section>
        ) : null}

        {activeClass ? (
          <section aria-labelledby="models-slots">
            <h2 id="models-slots" className="mb-2 text-sm font-semibold uppercase tracking-wide">
              {t("models.slots")}
            </h2>
            {/* No zebra: the row tint already means "inherited", a stripe would read as one more. */}
            <Table caption={t("models.slots")} zebra={false}>
              <TableHead>
                <TableHeaderCell>{t("models.slot")}</TableHeaderCell>
                <TableHeaderCell>{t("models.range")}</TableHeaderCell>
                <TableHeaderCell>
                  <Term name="ngsiLd">{t("models.kind")}</Term>
                </TableHeaderCell>
                <TableHeaderCell secondary>{t("models.unit")}</TableHeaderCell>
                <TableHeaderCell>{t("models.rules")}</TableHeaderCell>
                <TableHeaderCell secondary>{t("models.affordanceLabel")}</TableHeaderCell>
                <TableHeaderCell secondary>{t("models.from")}</TableHeaderCell>
              </TableHead>
              <TableBody>
                {/* Inherited fields too, greyed, with the type they come from (T-6106). */}
                {[...origins].map(([name, from]) => {
                  const slot: LinkmlSlot | undefined =
                    model.slots.find((candidate) => candidate.name === name) ?? inlineParts.get(name);
                  const inherited = from !== activeClass?.name;
                  return (
                    <TableRow key={name} className={
                        // The open row: a tint plus an accent bar on its first cell. An inherited row:
                        // a tint on every cell and a grey bar; the text keeps full contrast and "From <type>" says why.
                        name === selectedSlot
                          ? "[&>td]:bg-primary-soft [&>td:first-child]:border-l-4 [&>td:first-child]:border-primary"
                          : inherited
                            ? "[&>td]:bg-surface-muted [&>td:first-child]:border-l-4 [&>td:first-child]:border-border-strong"
                            : undefined
                      }
                      aria-current={name === selectedSlot ? "true" : undefined}>
                      {/* Below lg the Table paints the sticky name cell opaque in its own colour, which
                          beats the row tint: the cell takes the row's tint itself or it reads as a column. */}
                      <TableCell
                        primary
                        className={
                          name === selectedSlot ? "max-lg:bg-primary-soft!" : inherited ? "max-lg:bg-surface-muted!" : undefined
                        }
                      >
                        <Button
                          variant="ghost"
                          size="sm"
                          className="-mx-2.5 -my-1 h-auto! py-1 text-body! underline-offset-2 hover:underline"
                          onClick={() => openSlot(name)}
                          disabled={inlineParts.has(name)}
                          disabledReason={inlineParts.has(name) ? t("models.inlinePart") : undefined}
                        >
                          {name}
                        </Button>
                        {/* A phone hides the From column, so the origin rides under the name. */}
                        {inherited ? (
                          <span className="block px-2 text-body font-normal text-fg-muted sm:hidden">
                            {t("models.inheritedFrom", { type: from })}
                          </span>
                        ) : null}
                      </TableCell>
                      <TableCell>{slot ? word(fieldWord(slot, model)) : "—"}</TableCell>
                      <TableCell>{slot?.kind ?? DEFAULT_KIND}</TableCell>
                      <TableCell secondary>{unitSymbol(unitCode(slot?.unit)) || slot?.unit?.ucum_code || "—"}</TableCell>
                      {/* Required, multivalued and deprecated at a glance (owner 2026-11-01), words not colour alone. */}
                      <TableCell>
                        <span className="flex flex-wrap gap-1">
                          {slot?.required ? <Badge tone="info">{t("models.required")}</Badge> : null}
                          {slot?.multivalued ? <Badge>{t("models.multivalued")}</Badge> : null}
                          {slot?.deprecated ? <Badge tone="warning">{t("models.deprecatedBadge")}</Badge> : null}
                          {!slot?.required && !slot?.multivalued && !slot?.deprecated ? <span className="text-fg-muted">{t("models.optional")}</span> : null}
                        </span>
                      </TableCell>
                      <TableCell secondary>{slot ? t(`models.affordance.${slotAffordance(slot, model.enums.map((one) => one.name))}`) : "—"}</TableCell>
                      <TableCell secondary>
                        {inherited ? (
                          <Button
                            variant="ghost"
                            size="sm"
                            className="-mx-2.5 -my-1 h-auto! py-1 text-body! underline-offset-2 hover:underline"
                            onClick={() => setSelectedClass(from)}
                          >
                            {t("models.inheritedFrom", { type: from })}
                          </Button>
                        ) : (
                          t("models.ownField")
                        )}
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
            {/* One "Add field" button; the name is asked in a dialog, then the field's whole form opens
                (owner 2026-11-01: "when I want to add new, there should be add, and it should display
                everything in the form"). */}
            <div className="mt-2">
              <Button size="sm" icon={<Icon name="plus" className="size-4" />} onClick={() => {
                setSelectedSlot(null);
                setNewSlot("");
                setAddKind("Property");
                setAddingSlot(true);
              }}>
                {t("models.addSlot")}
              </Button>
            </div>
          </section>
        ) : (
          // Fields belong to a type: offering "New field" before one exists led nowhere (T-6254 judge).
          <p className="text-body text-fg-muted">{t("models.fieldsNeedType")}</p>
        )}

        {/* A field opens as its form in a dialog over the table, not a panel under it (owner 2026-11-01). */}
        <Dialog
          open={Boolean(activeSlot) || addingSlot}
          onOpenChange={(open) => {
            if (!open) {
              setSelectedSlot(null);
              setAddingSlot(false);
            }
          }}
          title={activeSlot?.name ?? t("models.addSlot")}
          closeLabel={t("app.close")}
          size="lg"
        >
          {/* One form (owner 2026-11-01): the name first, then everything else. The name is the field's
              key in every stored entity and payload, so it is set once and never changed here. */}
          {addingSlot && !activeSlot ? (
            <div className="mb-3 flex flex-col gap-3">
              {/* What the field holds first (owner 2026-11-01): a value, a link to another type, a place or a
                  text per language; a link takes the relationship form, every other kind the name. */}
              <RadioGroup
                name="add-kind"
                legend={t("models.addWhat")}
                value={addKind}
                options={ADD_KINDS.map((kind) => ({ value: kind, label: t(`models.addKind.${kind}`), description: t(`models.addKindHint.${kind}`) }))}
                onChange={setAddKind}
              />
              {addKind === "Relationship" && activeClass ? (
                <AddRelationshipForm
                  bare
                  source={source}
                  model={model}
                  klass={activeClass.name}
                  run={(operation) => {
                    const refused = runQuietly(operation);
                    if (refused === null) setAddingSlot(false);
                    return refused;
                  }}
                />
              ) : (
                <form
                  className="flex flex-col gap-2"
                  onSubmit={(event) => {
                    event.preventDefault();
                    const name = newSlot.trim();
                    if (name && activeClass && run({ op: "addSlot", name, class: activeClass.name })) {
                      if (addKind !== DEFAULT_KIND) setSlotField(name, "kind", addKind);
                      setAddingSlot(false);
                      setNewSlot("");
                      openSlot(name);
                    }
                  }}
                >
                  <Field id="new-slot-name" label={t("models.slotName")} help={t("models.slotNameHint")}>
                    <div className="flex gap-2">
                      <Input id="new-slot-name" aria-describedby={fieldIds("new-slot-name").help} value={newSlot} onChange={(event) => setNewSlot(event.target.value)} />
                      <Button type="submit" variant="primary" disabled={!newSlot.trim()} disabledReason={t("models.nameFirst")}>
                        {t("models.add")}
                      </Button>
                    </div>
                  </Field>
                  <p className="text-caption text-fg-muted">{t("models.slotNameFirst")}</p>
                </form>
              )}
            </div>
          ) : activeSlot ? (
            <div className="mb-3">
              <NameField
                id="slot-name"
                label={t("models.slotName")}
                help={t("models.fieldName.hint")}
                name={activeSlot.name}
                onRename={(to) => {
                  const done = run({ op: "renameSlot", name: activeSlot.name, to });
                  if (done) setSelectedSlot(to);
                  return done;
                }}
              />
            </div>
          ) : null}
          {activeSlot && activeSlotInheritedFrom ? (
            <div className="flex flex-col gap-2">
              <p className="text-fg-muted">
                {t("models.inheritedDefinedOn", { type: activeSlotInheritedFrom })}
              </p>
              <div>
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => setSelectedClass(activeSlotInheritedFrom)}
                >
                  {t("models.inheritedOpen", { type: activeSlotInheritedFrom })}
                </Button>
              </div>
            </div>
          ) : activeSlot ? (
            <div className="flex flex-col gap-3">
              <div className="grid gap-3 sm:grid-cols-2">
                <Field id="slot-range" label={t("models.range")}>
                  <Select
                    id="slot-range"
                    value={activeSlot.range ?? ""}
                    onChange={(event) => {
                      const range = event.target.value;
                      // A class is the other end of a relationship: never a one-sided slot (DM-64).
                      if (model.classes.some((klass) => klass.name === range)) {
                        setPrefill((last) => ({ target: range, key: (last?.key ?? 0) + 1 }));
                        // The form opens under the page: the field's dialog closes, or it would cover it.
                        setSelectedSlot(null);
                        return;
                      }
                      pickRange(activeSlot, range);
                    }}
                  >
                    <option value="">—</option>
                    <optgroup label={t("models.rangeBasic")}>
                      {RANGES.map((range) => {
                        const type = RANGE_TYPE.get(range);
                        return (
                          <option key={range} value={range}>
                            {type ? `${t(`spaces.fields.types.${type}`)} (${range})` : range}
                          </option>
                        );
                      })}
                    </optgroup>
                    {model.enums.length > 0 ? (
                      <optgroup label={t("models.rangeEnums")}>
                        {model.enums.map((entry) => (
                          <option key={entry.name} value={entry.name}>
                            {entry.name}
                          </option>
                        ))}
                      </optgroup>
                    ) : null}
                    {model.classes.length > 0 ? (
                      <optgroup label={t("models.rangeClasses")}>
                        {model.classes.map((klass) => (
                          <option key={klass.name} value={klass.name}>
                            {t("models.word.linkTo", { target: klass.name })}
                          </option>
                        ))}
                      </optgroup>
                    ) : null}
                  </Select>
                </Field>
                <Field id="slot-kind" label={t("models.kind")} help={t("models.kindHint")}>
                  <Select
                    id="slot-kind"
                    value={activeSlot.kind}
                    onChange={(event) =>
                      setSlotField(activeSlot.name, "kind", event.target.value as NgsiLdKind)
                    }
                  >
                    {NGSI_LD_KINDS.map((kind) => (
                      <option key={kind} value={kind}>
                        {kind}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Field id="slot-unit" label={t("models.unit")} help={t("models.unitHint")}>
                  <UnitPicker
                    id="slot-unit"
                    label={t("models.unit")}
                    labelled
                    clearable
                    value={unitCode(activeSlot.unit) ?? ""}
                    onChange={(code) => pickUnit(activeSlot, code)}
                  />
                </Field>
              </div>
              {unitUndo && unitUndo.slot === activeSlot.name && unitUndo.after === source ? (
                <Alert tone="info" role="status">
                  <span className="flex flex-wrap items-center justify-between gap-2">
                    <span>{t(`models.unitRule.${unitUndo.notice}`)}</span>
                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={() => {
                        onChange(unitUndo.before);
                        setUnitUndo(null);
                      }}
                    >
                      {t("models.unitRule.undo")}
                    </Button>
                  </span>
                </Alert>
              ) : null}
              <details className="rounded border border-border p-2" open={iriError !== null}>
                <summary className="cursor-pointer text-body font-medium">{t("models.advanced")}</summary>
                <div className="mt-2 grid gap-3 sm:grid-cols-2">
                <Field id="slot-uri" label={t("models.slotUri")} help={t("models.slotUriHint")}>
                  <Input
                    id="slot-uri"
                    value={iriDraft ?? activeSlot.slot_uri ?? ""}
                    onChange={(event) => setSlotIri(activeSlot.name, event.target.value)}
                  />
                </Field>
                {/* T-1088: which profiles take this slot, beside the rest of what it declares. */}
                <Field id="slot-subsets" label={t("models.slotSubsets")}>
                  <Input
                    id="slot-subsets"
                    value={(activeSlot.subsets ?? []).join(", ")}
                    onChange={(event) =>
                      run({
                        op: "setSlotSubsets",
                        name: activeSlot.name,
                        subsets: event.target.value.split(","),
                      })
                    }
                  />
                </Field>
                </div>
              </details>
              <div className="flex flex-wrap gap-4 text-sm">
                <Checkbox
                  label={t("models.required")}
                  checked={activeSlot.required === true}
                  onChange={(event) => setSlotField(activeSlot.name, "required", event.target.checked)}
                />
                <Checkbox
                  label={t("models.multivalued")}
                  checked={activeSlot.multivalued === true}
                  onChange={(event) => setSlotField(activeSlot.name, "multivalued", event.target.checked)}
                />
                <Checkbox
                  label={t("models.deprecatedKeep")}
                  checked={activeSlot.deprecated === true}
                  onChange={(event) => setSlotField(activeSlot.name, "deprecated", event.target.checked)}
                />
              </div>
              {activeSlot.upstream ? (
                <p className="text-xs text-surface-fg/70">
                  {t("models.upstream", { source: activeSlot.upstream })}
                </p>
              ) : null}
              {iriError ? (
                <Alert tone="danger" role="alert">
                  {iriError}
                </Alert>
              ) : null}
              {messagesFor(`slots.${activeSlot.name}`).map((diagnostic) => (
                <p key={diagnostic.message} role="status" className="text-caption text-warning">
                  {diagnostic.message}
                </p>
              ))}
            </div>
          ) : null}
        </Dialog>

        {activeClass ? (
          <RelationshipEditor
            source={source}
            model={model}
            klass={activeClass.name}
            run={runQuietly}
            prefill={prefill}
            withAdd={Boolean(prefill)}
          />
        ) : null}
        </>
        )}
      </div>
    </div>
  );
}

export { UPSTREAM_ANNOTATION };

/**
 * A class's title in each of the organisation's locales (DM-15): one row in the reader's language,
 * the others behind "Translations". Three empty Title inputs filled a phone's first screen (T-6254).
 */
export function TitleRow({
  locales,
  title,
  onChange,
}: {
  locales: string[];
  title?: Record<string, string>;
  onChange: (locale: string, value: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const first = locales.find((locale) => locale === i18n.language) ?? locales[0] ?? "en";
  const others = locales.filter((locale) => locale !== first);
  const input = (locale: string) => (
    <Field key={locale} id={`class-title-${locale}`} label={t("models.titleIn", { locale })} help={locale === first ? t("models.titleHint") : undefined}>
      <Input id={`class-title-${locale}`} value={title?.[locale] ?? ""} onChange={(event) => onChange(locale, event.target.value)} />
    </Field>
  );
  return (
    <div className="flex flex-col gap-2">
      {input(first)}
      {others.length > 0 ? (
        <details>
          <summary className="cursor-pointer text-caption text-fg-muted">
            {t("models.titleTranslations", { languages: others.join(", ") })}
          </summary>
          <div className="mt-2 grid gap-3 sm:grid-cols-2">{others.map(input)}</div>
        </details>
      ) : null}
    </div>
  );
}
