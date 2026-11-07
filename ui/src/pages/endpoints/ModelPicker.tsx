import { useEffect, useId, useMemo, useState, useRef } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { asManifests } from "../../api/manifest";
import { parseModel } from "../models/linkml";
import { Button, Checkbox, Field, Input, Select } from "../../components/ui";
import { FormHeading } from "../../components/forms/FormRoute";

export interface ClassConfig {
  ticked: boolean;
  slots: string[];
  readQ?: string;
  writable?: boolean;
  idPattern?: string;
  scope?: string;
  writeQ?: string;
}

export interface ModelPickerState {
  dataModelName?: string;
  dataModelVersion?: string;
  projectionName: string;
  selectedProjectionRef?: string;
  classes: Record<string, ClassConfig>;
}

export interface ModelPickerProps {
  project: string;
  spaceName: string;
  endpointName: string;
  disabled?: boolean;
  value: ModelPickerState;
  onChange: (next: ModelPickerState) => void;
  /**
   * The classes a hand-off named (AG-58): ticked once the model is read, when nothing is ticked
   * yet; an empty list ticks every class. A new endpoint over a modelled space is refused
   * without a ticked class, and the assistant's proposal must not stop there (T-0895).
   */
  handed?: string[];
}

const IDENTITY_SLOTS = ["id", "type"];

export function ModelPicker({
  project,
  spaceName,
  endpointName,
  disabled = false,
  value,
  onChange,
  handed,
}: ModelPickerProps): JSX.Element {
  const { t } = useTranslation();
  const hintId = useId();
  const [isDetached, setIsDetached] = useState(false);

  const modelsQuery = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "datamodels" } },
        }),
      ),
  });

  const projectionsQuery = useQuery({
    queryKey: queryKeys.list(project, "projections"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "projections" } },
        }),
      ),
  });

  const endpointsQuery = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });

  const matchingModel = useMemo(() => {
    const list = asManifests(modelsQuery.data?.items ?? []);
    return list.find((m) => {
      const sRef = (m.spec as { contextSpaceRef?: string })?.contextSpaceRef;
      return sRef === spaceName;
    });
  }, [modelsQuery.data, spaceName]);

  const rawLinkml = (matchingModel?.spec as { linkml?: string; source?: string })?.linkml;
  const needsSourceFetch =
    Boolean(matchingModel) && (!rawLinkml || !rawLinkml.includes("\n"));

  const sourceQuery = useQuery({
    queryKey: ["datamodels", project, matchingModel?.metadata.name, "source"],
    enabled: Boolean(matchingModel && needsSourceFetch),
    queryFn: async () => {
      const res = await globalThis.fetch(
        new Request(
          `${window.location.origin}/api/v1/projects/${encodeURIComponent(project)}/datamodels/${encodeURIComponent(matchingModel!.metadata.name)}/source`,
        ),
      );
      if (!res.ok) return "";
      return res.text();
    },
  });

  const modelSource = rawLinkml && rawLinkml.includes("\n") ? rawLinkml : sourceQuery.data ?? "";

  const parsedModel = useMemo(() => {
    if (!modelSource) return null;
    return parseModel(modelSource);
  }, [modelSource]);

  // A projection names the model's served major version ("1" of 1.8.0): the manifest refuses a
  // full semver, which the gateway never serves a schema under.
  const modelVersion = useMemo(() => {
    const spec = matchingModel?.spec as { version?: string | number } | undefined;
    return String(spec?.version ?? "1").split(".")[0] || "1";
  }, [matchingModel]);

  // Keep dataModel info synchronized in value
  useEffect(() => {
    if (matchingModel && (value.dataModelName !== matchingModel.metadata.name || value.dataModelVersion !== modelVersion)) {
      onChange({
        ...value,
        dataModelName: matchingModel.metadata.name,
        dataModelVersion: modelVersion,
      });
    }
  }, [matchingModel, modelVersion, value, onChange]);

  const handedOnce = useRef(false);
  useEffect(() => {
    if (!handed || handedOnce.current || !parsedModel) return;
    if (Object.values(value.classes).some((c) => c.ticked)) return;
    const wanted = parsedModel.classes.filter((c) => handed.length === 0 || handed.includes(c.name));
    if (wanted.length === 0) return;
    handedOnce.current = true;
    const classes = { ...value.classes };
    for (const c of wanted) {
      classes[c.name] = { ticked: true, slots: c.slots.filter((slot) => !IDENTITY_SLOTS.includes(slot)) };
    }
    onChange({ ...value, classes });
  }, [handed, parsedModel, value, onChange]);

  // Available projections for this space
  const availableProjections = useMemo(() => {
    const list = asManifests(projectionsQuery.data?.items ?? []);
    return list.filter((p) => {
      const sRef = (p.spec as { contextSpaceRef?: string })?.contextSpaceRef;
      return sRef === spaceName;
    });
  }, [projectionsQuery.data, spaceName]);

  // Find endpoints sharing the chosen projection
  const sharingEndpoints = useMemo(() => {
    if (!value.selectedProjectionRef) return [];
    const eps = asManifests(endpointsQuery.data?.items ?? []);
    return eps
      .filter((ep) => {
        const pRef = (ep.spec as { projectionRef?: { name?: string } })?.projectionRef?.name;
        // The endpoint being edited is not somebody else: an endpoint already on a shared
        // projection used to read "Shared by: air-public" about itself, which says the opposite
        // of what the line is for — that a change here changes another endpoint too.
        return pRef === value.selectedProjectionRef && ep.metadata.name !== endpointName;
      })
      .map((ep) => ep.metadata.name);
  }, [endpointsQuery.data, endpointName, value.selectedProjectionRef]);

  const isReadOnly = Boolean(value.selectedProjectionRef && !isDetached);
  // Why a control is closed (ui-writing rule 7): a proposal on its way, or a shared projection.
  const locked = disabled ? t("app.busyReason") : isReadOnly ? t("endpoints.picker.readOnly") : undefined;

  /** The first class of the model, which is the one an example ticks (T-2258). */
  const firstClass = parsedModel?.classes?.[0];
  const nothingTicked =
    firstClass !== undefined && !Object.values(value.classes).some((config) => config.ticked);

  const handleSelectProjection = (projName: string) => {
    if (!projName) {
      // Draw new
      setIsDetached(false);
      onChange({
        ...value,
        selectedProjectionRef: undefined,
        projectionName: value.projectionName || endpointName || "projection",
      });
      return;
    }
    const found = availableProjections.find((p) => p.metadata.name === projName);
    if (!found) return;

    setIsDetached(false);
    const spec = found.spec as {
      classes?: Array<{ name: string; slots?: string[] }>;
      filter?: { q?: string };
    };
    const nextClasses: Record<string, ClassConfig> = {};
    for (const c of spec.classes ?? []) {
      nextClasses[c.name] = {
        ticked: true,
        slots: c.slots ?? [],
        readQ: spec.filter?.q,
      };
    }

    onChange({
      ...value,
      selectedProjectionRef: projName,
      projectionName: projName,
      classes: nextClasses,
    });
  };

  const handleDetach = () => {
    setIsDetached(true);
    onChange({
      ...value,
      selectedProjectionRef: undefined,
      projectionName: endpointName || `${value.projectionName}-copy`,
    });
  };

  const toggleClass = (className: string, allNonIdSlots: string[]) => {
    if (isReadOnly || disabled) return;
    const current = value.classes[className];
    const isCurrentlyTicked = Boolean(current?.ticked);
    const nextClasses = { ...value.classes };

    if (isCurrentlyTicked) {
      delete nextClasses[className];
    } else {
      nextClasses[className] = {
        ticked: true,
        slots: [...allNonIdSlots],
        writable: current?.writable,
        idPattern: current?.idPattern,
        scope: current?.scope,
        writeQ: current?.writeQ,
        readQ: current?.readQ,
      };
    }
    onChange({ ...value, classes: nextClasses });
  };

  const toggleSlot = (className: string, slotName: string) => {
    if (isReadOnly || disabled) return;
    const current = value.classes[className];
    if (!current?.ticked) return;
    const hasSlot = current.slots.includes(slotName);
    const nextSlots = hasSlot
      ? current.slots.filter((s) => s !== slotName)
      : [...current.slots, slotName];
    onChange({
      ...value,
      classes: {
        ...value.classes,
        [className]: {
          ...current,
          slots: nextSlots,
        },
      },
    });
  };

  const updateClassConfig = (className: string, patch: Partial<ClassConfig>) => {
    if (isReadOnly || disabled) return;
    const current = value.classes[className] ?? { ticked: true, slots: [] };
    onChange({
      ...value,
      classes: {
        ...value.classes,
        [className]: {
          ...current,
          ...patch,
        },
      },
    });
  };

  if (modelsQuery.isPending) {
    return <p className="text-caption text-fg-muted">{t("app.loading")}</p>;
  }

  if (!matchingModel) {
    return (
      <div className="rounded border border-border bg-surface-subtle p-3 text-caption text-fg-muted">
        {t("endpoints.picker.noModel")}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3 rounded border border-border bg-surface-subtle p-3">
      <div>
        <FormHeading sub className="text-body font-semibold text-fg">{t("endpoints.picker.title")}</FormHeading>
        <p id={hintId} className="text-caption text-fg-muted">{t("endpoints.picker.hint")}</p>
      </div>

      <div className="flex flex-wrap items-end gap-3">
        <Field
          id="projection-reuse"
          label={t("endpoints.picker.reuse")}
          help={t("endpoints.picker.reuseHelp")}
          className="min-w-48"
        >
          <Select
            id="projection-reuse"
            disabled={disabled}
            title={disabled ? t("app.busyReason") : undefined}
            value={value.selectedProjectionRef ?? ""}
            onChange={(e) => handleSelectProjection(e.target.value)}
          >
            <option value="">{t("endpoints.picker.drawNew")}</option>
            {availableProjections.map((p) => (
              <option key={p.metadata.name} value={p.metadata.name}>
                {p.metadata.name}
              </option>
            ))}
          </Select>
        </Field>

        {value.selectedProjectionRef ? (
          <div className="flex flex-wrap items-center gap-2 pb-1.5">
            {sharingEndpoints.length > 0 ? (
              <span className="text-caption text-fg-muted">
                {t("endpoints.picker.sharedBy", { endpoints: sharingEndpoints.join(", ") })}
              </span>
            ) : null}
            {!isDetached ? (
              <Button size="sm" onClick={handleDetach}>
                {t("endpoints.picker.detach")}
              </Button>
            ) : null}
          </div>
        ) : (
          <Field
            id="projection-name"
            label={t("endpoints.picker.projectionName")}
            help={t("endpoints.picker.projectionNameHelp")}
            className="min-w-48 flex-1"
          >
            <Input
              id="projection-name"
              disabled={disabled || isReadOnly}
              title={locked}
              value={value.projectionName}
              onChange={(e) => onChange({ ...value, projectionName: e.target.value })}
              className="font-mono"
            />
          </Field>
        )}
      </div>

      {/*
        What the endpoint publishes is a choice nobody can make for the person, and until it is
        made the check refuses the whole form. Saying so here, beside the ticks, is where the
        person is looking — before T-2258 the sentence arrived only after Check, under the button,
        and a form filled entirely from its own examples still could not be checked. The example
        ticks the first class, exactly as a hand-off does (T-0895), and never more than one.
      */}
      {nothingTicked ? (
        <div
          role="note"
          className="flex flex-wrap items-center gap-2 rounded border border-border bg-surface px-2.5 py-2 text-caption text-fg"
        >
          <span className="min-w-0 flex-1">{t("endpoints.picker.nothingTicked")}</span>
          <Button
            size="sm"
            variant="secondary"
            disabled={disabled || isReadOnly}
            title={locked}
            onClick={() =>
              toggleClass(
                firstClass.name,
                firstClass.slots.filter((slot) => !IDENTITY_SLOTS.includes(slot)),
              )
            }
          >
            {t("form.useExample")}
          </Button>
        </div>
      ) : null}

      {/* Every box below is one choice of the panel's hint (T-3852, field-hint). */}
      <fieldset aria-describedby={hintId} className="flex flex-col gap-3 divide-y divide-border">
        <legend className="sr-only">{t("endpoints.picker.title")}</legend>
        {(parsedModel?.classes ?? []).map((cls) => {
          const cfg = value.classes[cls.name];
          const isTicked = Boolean(cfg?.ticked);
          const nonIdSlots = cls.slots.filter((s) => !IDENTITY_SLOTS.includes(s));
          const isIdentityOnly = isTicked && cfg.slots.length === 0;

          return (
            <div key={cls.name} className="pt-2 flex flex-col gap-2">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <Checkbox
                  className="font-medium text-caption"
                  // The class name is the accessible name: the hint after it says what is ticked
                  // inside the class, which is a second sentence about the same box rather than
                  // part of what the box is called.
                  aria-label={cls.name}
                  label={<span className="font-mono">{cls.name}</span>}
                  hint={isIdentityOnly ? `(${t("endpoints.picker.identityOnly")})` : undefined}
                  disabled={disabled || isReadOnly}
                  title={locked}
                  checked={isTicked}
                  onChange={() => toggleClass(cls.name, nonIdSlots)}
                />

                {isTicked ? (
                  <div className="flex flex-wrap items-center gap-3">
                    <Checkbox
                      className="text-caption"
                      // The class name tells one row's "Writable" from the next one's; the word
                      // itself is in the name, so what is read matches what is on the screen
                      // (WCAG 2.5.3), in the language the person is reading.
                      aria-label={`${cls.name} ${t("endpoints.picker.writable")}`}
                      label={t("endpoints.picker.writable")}
                      disabled={disabled || isReadOnly}
                      title={locked}
                      checked={Boolean(cfg.writable)}
                      onChange={(e) => updateClassConfig(cls.name, { writable: e.target.checked })}
                    />

                    <div className="w-40">
                      <Input
                        aria-label={`${cls.name} ${t("endpoints.picker.readQ")}`}
                        placeholder={t("endpoints.picker.readQ")}
                        disabled={disabled || isReadOnly}
                        title={locked}
                        value={cfg.readQ ?? ""}
                        onChange={(e) => updateClassConfig(cls.name, { readQ: e.target.value })}
                        className="font-mono"
                      />
                    </div>
                  </div>
                ) : null}
              </div>

              {isTicked && cfg.writable ? (
                <div className="ml-6 flex flex-wrap gap-2 rounded border border-border bg-surface p-2">
                  <div className="min-w-48 flex-1">
                    <Input
                      aria-label={`${cls.name} ${t("endpoints.picker.idPattern")}`}
                      placeholder={t("endpoints.picker.idPattern")}
                      disabled={disabled || isReadOnly}
                      title={locked}
                      value={cfg.idPattern ?? ""}
                      onChange={(e) => updateClassConfig(cls.name, { idPattern: e.target.value })}
                      className="font-mono"
                    />
                  </div>
                  <div className="w-40">
                    <Input
                      aria-label={`${cls.name} ${t("endpoints.picker.scope")}`}
                      placeholder={t("endpoints.picker.scope")}
                      disabled={disabled || isReadOnly}
                      title={locked}
                      value={cfg.scope ?? ""}
                      onChange={(e) => updateClassConfig(cls.name, { scope: e.target.value })}
                      className="font-mono"
                    />
                  </div>
                  <div className="w-40">
                    <Input
                      aria-label={`${cls.name} ${t("endpoints.picker.q")}`}
                      placeholder={t("endpoints.picker.q")}
                      disabled={disabled || isReadOnly}
                      title={locked}
                      value={cfg.writeQ ?? ""}
                      onChange={(e) => updateClassConfig(cls.name, { writeQ: e.target.value })}
                      className="font-mono"
                    />
                  </div>
                </div>
              ) : null}

              {isTicked ? (
                <div className="ml-6 flex flex-wrap gap-x-4 gap-y-1">
                  {IDENTITY_SLOTS.map((idSlot) => (
                    <Checkbox
                      key={idSlot}
                      className="text-caption text-fg-muted"
                      aria-label={`${cls.name}.${idSlot}`}
                      // Every entity carries its id and its type, so these two are not a choice
                      // and the tick says so rather than inviting one. The reason is read with
                      // the box, which "dimmed and ticked" on its own never explained (UI-44).
                      // `title` and not a visible hint: the sentence beside each of the two
                      // boxes of every class would be the loudest text in the list.
                      title={t("endpoints.picker.identityAlways")}
                      label={<span className="font-mono">{idSlot}</span>}
                      checked
                      readOnly
                      disabled
                    />
                  ))}
                  {nonIdSlots.map((slot) => {
                    const slotTicked = cfg.slots.includes(slot);
                    return (
                      <Checkbox
                        key={slot}
                        className="text-caption"
                        aria-label={`${cls.name}.${slot}`}
                        label={<span className="font-mono">{slot}</span>}
                        disabled={disabled || isReadOnly}
                        title={locked}
                        checked={slotTicked}
                        onChange={() => toggleSlot(cls.name, slot)}
                      />
                    );
                  })}
                </div>
              ) : null}
            </div>
          );
        })}
      </fieldset>
    </div>
  );
}
