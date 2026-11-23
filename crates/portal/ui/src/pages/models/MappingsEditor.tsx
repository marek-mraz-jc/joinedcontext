import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { parse as parseYaml } from "yaml";
import { useProposal } from "../../api/proposal";
import type { Change } from "../../api/manifest";
import { parseModel } from "./linkml";
import { UnitPicker } from "../../components/pickers/UnitPicker";
import type { LinkmlModel } from "./linkml";
import {
  autoAlign,
  laneOf,
  nativeBlocks,
  toTransformationSpec,
  transform,
  unfilledRequired,
} from "./mapping";
import type { Alignment, Derivation } from "./mapping";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { DNS1123 } from "../../schemas/kinds";
import { RecordView,
  Alert,
  Badge,
  Button,
  Field,
  Input,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Textarea,
} from "../../components/ui";

/**
 * The Mappings tab: two models side by side and the derivations between them (T-0223, DM-33,
 * DM-38, DM-40).
 *
 * The editor's job is to make a `TransformationSpecification` that a person can review. It
 * never invents a derivation it cannot justify — every filled row says where its guess came
 * from — and it never claims a mapping is correct: the browser runs the invertible subset over
 * one example so the author can see the shape, and CI runs the golden tests through Bento and
 * Model Tools, which is the answer that counts (DM-39, DM-52).
 *
 * Nothing here writes to the platform. Saving a Mapping is a repository change and travels the
 * lane like any other, which is why the lane is on screen while the mapping is being written
 * rather than after it is submitted (DM-38, CC-32).
 */

/** One model the project publishes, as this editor needs it. */
export interface MappingModel {
  name: string;
  version: string;
  /** The LinkML source, which is the one place the slots come from. */
  source: string;
}

export interface MappingsEditorProps {
  /** The models to map between; two are needed before anything can be aligned. */
  models: MappingModel[];
  /** SSSOM alignment sets, used only to pre-fill (DM-42). */
  alignments?: Alignment[];
  /** Called whenever the specification changes, for the page that saves it. */
  onChange?: (spec: string) => void;
  /** The project the Mapping is proposed into; without one the tab only aligns (T-0795). */
  project?: string;
  /** The space a model belongs to: a Mapping belongs to the same one (DM-33). */
  spaceOf?: (modelName: string) => string | undefined;
  /** The Change the proposal opened, for the page that shows it. */
  onProposed?: (change: Change) => void;
}

/** The served major of a model version: what a `DataModelRef` carries (DM-22, DM-33). */
function major(version: string): string {
  return version.split(".")[0] || "1";
}

/**
 * The `kind: Mapping` the editor's alignment describes (DM-33, DM-38, DM-39). The
 * transformation is the same YAML the tab shows, read back as the object the manifest carries.
 */
export function mappingManifest(
  project: string,
  name: string,
  space: string,
  source: MappingModel,
  target: MappingModel,
  transformation: string,
  native: { targetSlot: string; language: "bloblang"; source: string }[],
  example: string,
  produced: Record<string, unknown>,
) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Mapping",
    metadata: { name, namespace: project },
    spec: {
      contextSpaceRef: space,
      source: { name: source.name, version: major(source.version) },
      target: { name: target.name, version: major(target.version) },
      transformation: parseYaml(transformation) as unknown,
      ...(native.length > 0 ? { native } : {}),
      // DM-39: a Mapping carries at least one golden test. The pair is the example this tab
      // shows and what it produced, committed beside the manifest.
      tests: [{ input: `./tests/${name}.input.json`, expect: `./tests/${name}.expect.json` }],
    },
    // The two documents the test reads, committed in the same change as the manifest that names
    // them (DM-39, API/01 §4): what the preview shows is what a reviewer approves.
    files: {
      [`./tests/${name}.input.json`]: example,
      [`./tests/${name}.expect.json`]: `${JSON.stringify(produced, null, 2)}\n`,
    },
  };
}

const EXAMPLE = `{\n  "id": "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:st-1",\n  "type": "AirQualityObserved"\n}`;

export function MappingsEditor({
  models,
  alignments = [],
  onChange,
  project,
  spaceOf,
  onProposed,
}: MappingsEditorProps): JSX.Element {
  const { t } = useTranslation();
  const [sourceName, setSourceName] = useState(models[0]?.name ?? "");
  const [targetName, setTargetName] = useState(models[1]?.name ?? models[0]?.name ?? "");
  const [derivations, setDerivations] = useState<Derivation[] | undefined>(undefined);
  const [example, setExample] = useState(EXAMPLE);
  // The name the pair makes until the person types another (T-1547): a project may map one pair
  // twice, and a mapping is not bound to the one name its models give it.
  const [typedName, setTypedName] = useState<string | null>(null);
  const name = typedName ?? `${sourceName}-to-${targetName}`;
  const badName = name.length > 63 || !new RegExp(DNS1123).test(name);

  const source = useParsed(models, sourceName);
  const target = useParsed(models, targetName);

  // The alignment is recomputed whenever the pair changes, and the author's own edits are kept
  // only while the pair does not: a derivation onto a slot the new target does not have is a
  // derivation onto nothing.
  const aligned = useMemo(
    () => autoAlign(source, target, alignments),
    [source, target, alignments],
  );
  const rows = derivations ?? aligned;

  const update = (targetSlot: string, change: Partial<Derivation>) => {
    const next = rows.map((row) =>
      row.target === targetSlot ? { ...row, ...change, origin: "manual" as const } : row,
    );
    setDerivations(next);
    onChange?.(toTransformationSpec(next, className(source), className(target)));
  };

  const missingRequired = unfilledRequired(target, rows);
  const lane = laneOf(rows);
  const blocks = nativeBlocks(rows);

  // A Mapping is a manifest like any other: it is proposed, reviewed and merged (CC-32, DM-33).
  const proposal = useProposal(project ?? "", "mappings", (change) => onProposed?.(change));
  const sourceModel = models.find((model) => model.name === sourceName);
  const targetModel = models.find((model) => model.name === targetName);
  const space = spaceOf?.(targetName) ?? spaceOf?.(sourceName);
  const parsedExample = useMemo(() => {
    try {
      const value: unknown = JSON.parse(example);
      return typeof value === "object" && value !== null
        ? { value: value as Record<string, unknown> }
        : { error: t("mappings.exampleNotAnObject") };
    } catch {
      return { error: t("mappings.exampleNotJson") };
    }
  }, [example, t]);
  const result = parsedExample.value ? transform(parsedExample.value, rows) : undefined;

  // The example is the golden test's input, so a proposal needs one that parses: an example
  // that is not an object would commit a test nothing can read (DM-39).
  const refusal =
    sourceName === targetName
      ? t("mappings.sameModel")
      : badName
        ? t("mappings.badName")
      : parsedExample.error !== undefined
        ? parsedExample.error
        : missingRequired.length > 0
          ? t("mappings.unfilled", { slots: missingRequired.join(", ") })
          : space === undefined
            ? t("mappings.noSpace")
            : null;

  return (
    <div className="flex flex-col gap-4">
      <section aria-labelledby="mappings-pair" className="flex flex-wrap items-end gap-3">
        <h2 id="mappings-pair" className="sr-only">
          {t("mappings.pair")}
        </h2>
        <Field id="mappings-source" label={t("mappings.source")} help={t("mappings.sourceHelp")}>
          <Select
            id="mappings-source"
            value={sourceName}
            onChange={(event) => {
              setSourceName(event.target.value);
              setDerivations(undefined);
            }}
          >
            {models.map((model) => (
              <option key={model.name} value={model.name}>
                {model.name} {model.version}
              </option>
            ))}
          </Select>
        </Field>
        <span aria-hidden="true" className="pb-2 text-lg text-fg-muted">
          →
        </span>
        <Field id="mappings-target" label={t("mappings.target")} help={t("mappings.targetHelp")}>
          <Select
            id="mappings-target"
            value={targetName}
            onChange={(event) => {
              setTargetName(event.target.value);
              setDerivations(undefined);
            }}
          >
            {models.map((model) => (
              <option key={model.name} value={model.name}>
                {model.name} {model.version}
              </option>
            ))}
          </Select>
        </Field>
        {project ? (
          <Field id="mappings-name" label={t("mappings.name")} help={t("mappings.nameHelp")}>
            <Input
              id="mappings-name"
              value={name}
              autoComplete="off"
              spellCheck={false}
              aria-invalid={badName || undefined}
              onChange={(event) => setTypedName(event.target.value)}
            />
          </Field>
        ) : null}
        <div className="flex items-center gap-1.5 pb-1 text-body">
          <span className="text-fg-muted">{t("mappings.lane")}</span>
          <LifecycleBadge kind="lane" value={lane} />
        </div>
        {project ? (
          <Button
            size="sm"
            variant="primary"
            // UI-44, T-1743: the reason travels with the button. A hard-disabled Propose left the
            // tab order and the refusal stood in a paragraph somewhere below it, so a reader who
            // could not see it greyed out never learned what was missing.
            disabled={refusal !== null}
            disabledReason={refusal ?? undefined}
            loading={proposal.mutation.isPending}
            onClick={() => {
              if (refusal !== null || !sourceModel || !targetModel || !space) {
                return;
              }
              proposal.mutation.mutate({
                body: mappingManifest(
                  project,
                  name,
                  space,
                  sourceModel,
                  targetModel,
                  toTransformationSpec(rows, className(source), className(target)),
                  blocks,
                  example,
                  result?.output ?? {},
                ),
                create: true,
              });
            }}
          >
            {t("mappings.propose")}
          </Button>
        ) : null}
      </section>

      {project ? (
        <>
          {refusal && missingRequired.length === 0 ? (
            <p role="status" className="text-body text-fg-muted">
              {refusal}
            </p>
          ) : refusal ? null : (
            <p className="text-body text-fg-muted">{t("mappings.goldenHint")}</p>
          )}
          {proposal.error ? (
            <Alert role="alert" tone="danger">
              {proposal.error}
            </Alert>
          ) : null}
        </>
      ) : null}

      {missingRequired.length > 0 ? (
        <Alert role="alert" tone="danger">
          {t("mappings.unfilled", { slots: missingRequired.join(", ") })}
        </Alert>
      ) : null}

      <section aria-labelledby="mappings-canvas">
        <h2 id="mappings-canvas" className="text-title font-semibold text-fg">
          {t("mappings.canvas")}
        </h2>
        <Table caption={t("mappings.canvas")} className="mt-2">
          <TableHead>
            <TableHeaderCell>{t("mappings.targetSlot")}</TableHeaderCell>
            <TableHeaderCell>{t("mappings.sourceSlot")}</TableHeaderCell>
            <TableHeaderCell>{t("mappings.origin")}</TableHeaderCell>
            <TableHeaderCell>{t("mappings.unit")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {rows.map((row) => {
              const targetSlot = target.slots.find((slot) => slot.name === row.target);
              return (
                <TableRow key={row.target}>
                  <TableCell primary>
                    <span className="font-mono">
                      {row.target}
                      {targetSlot?.required ? <span aria-hidden="true"> *</span> : null}
                    </span>
                  </TableCell>
                  <TableCell>
                    <label className="sr-only" htmlFor={`from-${row.target}`}>
                      {t("mappings.sourceFor", { slot: row.target })}
                    </label>
                    <Select
                      id={`from-${row.target}`}
                      value={row.populatedFrom ?? ""}
                      onChange={(event) =>
                        update(row.target, { populatedFrom: event.target.value || undefined })
                      }
                    >
                      <option value="">{t("mappings.unmapped")}</option>
                      {source.slots.map((slot) => (
                        <option key={slot.name} value={slot.name}>
                          {slot.name}
                        </option>
                      ))}
                    </Select>
                  </TableCell>
                  <TableCell>
                    {row.native?.trim() ? (
                      <Badge
                        tone="warning"
                        data-testid={`native-${row.target}`}
                      >
                        {t("mappings.native")}
                      </Badge>
                    ) : (
                      <span className="text-caption text-fg-muted">
                        {t(`mappings.originOf.${row.origin}`)}
                      </span>
                    )}
                  </TableCell>
                  <TableCell>
                    {row.unitConversion ? (
                      <span className="flex items-center gap-1">
                        <UnitPicker
                          id={`unit-${row.target}`}
                          label={t("mappings.unitFor", { slot: row.target })}
                          value={row.unitConversion.toUnit ?? ""}
                          onChange={(code) =>
                            update(row.target, {
                              unitConversion: {
                                ...row.unitConversion,
                                factor: row.unitConversion?.factor ?? 1,
                                toUnit: code,
                              },
                            })
                          }
                        />
                        <label className="sr-only" htmlFor={`factor-${row.target}`}>
                          {t("mappings.factorFor", { slot: row.target })}
                        </label>
                        <Input
                          id={`factor-${row.target}`}
                          type="number"
                          step="any"
                          className="w-24"
                          value={row.unitConversion.factor}
                          onChange={(event) =>
                            update(row.target, {
                              unitConversion: {
                                ...row.unitConversion,
                                factor: Number(event.target.value),
                                toUnit: row.unitConversion?.toUnit,
                              },
                            })
                          }
                        />
                      </span>
                    ) : null}
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </section>

      <section aria-labelledby="mappings-golden" className="flex flex-col gap-2">
        <h2 id="mappings-golden" className="text-title font-semibold text-fg">
          {t("mappings.golden")}
        </h2>
        <Field id="mappings-input" label={t("mappings.input")} help={t("mappings.inputHelp")}>
          <Textarea
            id="mappings-input"
            className="h-32 font-mono text-xs"
            value={example}
            onChange={(event) => setExample(event.target.value)}
          />
        </Field>
        {parsedExample.error ? (
          <Alert role="alert" tone="danger">
            {parsedExample.error}
          </Alert>
        ) : (
          <output aria-label={t("mappings.output")} className="block">
            <RecordView value={result?.output ?? {}} label={t("codeBlock.json")} className="rounded border border-border bg-surface-subtle p-2" />
          </output>
        )}
        {result && result.unchecked.length > 0 ? (
          // DM-38: the browser cannot run Bloblang, and a preview that quietly omitted these
          // would read as "this slot is empty" rather than "nobody checked this slot".
          <p className="text-body">{t("mappings.unchecked", { slots: result.unchecked.join(", ") })}</p>
        ) : null}
        {result && result.missing.length > 0 ? (
          <p className="text-body text-fg-muted">
            {t("mappings.missing", { slots: result.missing.join(", ") })}
          </p>
        ) : null}
      </section>

      {blocks.length > 0 ? (
        <p className="text-body text-fg-muted">{t("mappings.laneRaised", { count: blocks.length })}</p>
      ) : null}
    </div>
  );
}

/** The named model, parsed; an empty model when nothing is selected yet. */
function useParsed(models: MappingModel[], name: string): LinkmlModel {
  return useMemo(() => {
    const model = models.find((candidate) => candidate.name === name);
    return parseModel(model?.source ?? "");
  }, [models, name]);
}

/** The one class a model declares, which is what a class derivation is written against. */
function className(model: LinkmlModel): string {
  return model.classes[0]?.name ?? model.name ?? "Entity";
}
