import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { Manifest } from "../../../api/manifest";
import { TypePicker } from "../../pickers/TypePicker";
import { filterSlotsOf, useModelSource } from "../../entities/filters";
import { Badge, Checkbox, Field, InlineError, Input, Select, Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow, TableRowHeaderCell } from "../../ui";
import { baseOf, CORE_ATTRIBUTES, ICAL_FIELDS, ICAL_PRESETS, presetSources, unmappedRequired, withPreset, withSource, withType } from "./icalMapping";
import type { IcalMapping, IcalSource } from "./icalMapping";
import { CORE_GEOMETRIES, FEED_PRESETS, FEED_ROLES, feedPresetRoles, MAX_COLUMNS, MAX_POPUP, TABLE_PAGE_LENGTHS, unmappedRoles, withMapRole, withPopup, withRole, withTable } from "./representationOutputs";
import type { FeedSource, MapSource } from "./representationOutputs";

/** A remap written in YAML, shown as it is: the table edits plain attribute names only. */
function remapText(source: IcalSource): string {
  return Object.entries(source)
    .map(([key, value]) => `${key}: ${typeof value === "string" ? value : JSON.stringify(value)}`)
    .join(", ");
}

/**
 * The attributes a role or field can come from: the type's slots in the space's model, which the
 * server lists only to a person who may read it, and the attributes every entity has. A value
 * written before stays offered, so opening the form never drops it; the save checks it again
 * against the model and names it when it does not fit (EP-125, API/11 EP-159).
 */
export function useAttributeChoices(project: string, model: Manifest | undefined, type: string): string[] {
  const slots = filterSlotsOf(useModelSource(project, model), type);
  return [...new Set([...slots.map((slot) => slot.name), ...CORE_ATTRIBUTES])].sort();
}

export interface SourceEditorProps<S> {
  /** The id prefix of this source's controls. */
  id: string;
  value: S;
  onChange: (next: S) => void;
  project: string;
  space: string;
  model: Manifest | undefined;
  frozen: boolean;
}

/**
 * One model class of a calendar output: the type, the preset it starts from and a row per
 * RFC 5545 field, its attribute picked from the type's slots, the preset's attribute shown where
 * the person picked none (EP-125, T-4900).
 */
export function IcalSourceEditor({ id, value, onChange, project, space, model, frozen }: SourceEditorProps<IcalMapping>): JSX.Element {
  const { t } = useTranslation();
  const type = value.type ?? "Event";
  const base = baseOf(value);
  const fromPreset = presetSources(base);
  const choices = useAttributeChoices(project, model, type);
  const missing = unmappedRequired(value);
  // `tidy` leaves the default out; a source in a list always names its type.
  const write = (next: IcalMapping | undefined): void => onChange({ type, ...(next ?? {}) });

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap gap-4">
        <label className="flex min-w-48 flex-1 flex-col gap-1 text-caption font-semibold">
          {t("endpoints.ical.type")}
          {frozen ? (
            <span className="font-mono text-body font-normal">{type}</span>
          ) : (
            // The classes of the space's model and what it imports, from the models the server
            // lists to this caller (T-4947); the save checks the type against the model again.
            <TypePicker
              id={`${id}-type`}
              label={t("endpoints.ical.type")}
              labelled
              aria-describedby={`${id}-hint`}
              project={project}
              space={space || undefined}
              value={[type]}
              onChange={(types) => write(withType(value, types[0] ?? "Event"))}
            />
          )}
        </label>
        <label className="flex min-w-48 flex-1 flex-col gap-1 text-caption font-semibold">
          {t("endpoints.ical.start")}
          {frozen ? (
            <span className="text-body font-normal">{base ? t(`endpoints.ical.preset.${base}`) : t("endpoints.ical.presetOwn")}</span>
          ) : (
            <Select
              id={`${id}-preset`}
              aria-describedby={`${id}-hint`}
              value={value.preset ?? (base === undefined ? "" : "default")}
              onChange={(event) => {
                // "The default" is offered only while nothing is written: picking it changes nothing.
                if (event.currentTarget.value !== "default") write(withPreset(value, event.currentTarget.value));
              }}
            >
              {value.preset === undefined && base !== undefined ? <option value="default">{t("endpoints.ical.presetDefault")}</option> : null}
              {ICAL_PRESETS.map((preset) => (
                <option key={preset} value={preset}>
                  {t(`endpoints.ical.preset.${preset}`)}
                </option>
              ))}
              <option value="">{t("endpoints.ical.presetOwn")}</option>
            </Select>
          )}
        </label>
      </div>
      <p id={`${id}-hint`} className="text-caption text-fg-muted">
        {t("endpoints.outputs.type.hint")} {t("endpoints.outputs.presetHint")}
      </p>
      {missing.length > 0 ? <InlineError>{t("endpoints.ical.missing", { fields: missing.join(", ") })}</InlineError> : null}
      <Table stack caption={t("endpoints.ical.caption")} zebra={false}>
        <TableHead>
          <TableHeaderCell>{t("endpoints.ical.field")}</TableHeaderCell>
          <TableHeaderCell>{t("endpoints.ical.attribute")}</TableHeaderCell>
        </TableHead>
        <TableBody>
          {ICAL_FIELDS.map(({ field, required, fixed }) => {
            const picked = value.mapping?.[field];
            const inputId = `${id}-${field}`;
            return (
              <TableRow key={field}>
                <TableRowHeaderCell className="font-mono">
                  <span className="flex flex-col items-start gap-1">
                    <label htmlFor={fixed ? undefined : inputId}>{field}</label>
                    {required ? <Badge tone="neutral">{t("endpoints.ical.requiredBadge")}</Badge> : null}
                  </span>
                </TableRowHeaderCell>
                <TableCell className="w-full">
                  {fixed ? (
                    <span className="text-fg-muted">{t("endpoints.ical.fixed", { attribute: fixed })}</span>
                  ) : picked !== undefined && typeof picked !== "string" ? (
                    <span className="font-mono text-caption" title={t("endpoints.ical.remap")}>
                      {remapText(picked)}
                    </span>
                  ) : frozen ? (
                    <span className="font-mono text-caption">{picked ?? fromPreset[field] ?? t("endpoints.ical.unmapped")}</span>
                  ) : (
                    <AttributeSelect
                      id={inputId}
                      aria-describedby={`${id}-hint`}
                      value={picked ?? ""}
                      choices={choices}
                      empty={fromPreset[field] ? t("endpoints.outputs.fromPreset", { attribute: fromPreset[field] }) : t("endpoints.ical.unmapped")}
                      invalid={missing.includes(field)}
                      onChange={(attribute) => write(withSource(value, field, attribute))}
                    />
                  )}
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
}

/** A dropdown of the attributes a field may come from; the empty option leaves it to the preset. */
export function AttributeSelect({
  id,
  value,
  choices,
  "aria-describedby": describedBy,
  empty,
  invalid,
  onChange,
}: {
  id: string;
  /** The hint that says where the field's attribute comes from (UI-16); `Field` passes its own. */
  "aria-describedby"?: string;
  value: string;
  choices: string[];
  empty: string;
  invalid?: boolean;
  onChange: (attribute: string) => void;
}): JSX.Element {
  const options = value && !choices.includes(value) ? [value, ...choices] : choices;
  return (
    <Select id={id} value={value} aria-describedby={describedBy} aria-invalid={invalid ? "true" : undefined} onChange={(event) => onChange(event.currentTarget.value)}>
      <option value="">{empty}</option>
      {options.map((choice) => (
        <option key={choice} value={choice}>
          {choice}
        </option>
      ))}
    </Select>
  );
}

/**
 * One model class of a feed output: the type, the preset it starts from and a row per entry role,
 * its attribute picked from the type's slots, the preset's shown where the person picked none
 * (EP-150, API/11 EP-159).
 */
export function FeedSourceEditor({ id, value, onChange, project, space, model, frozen }: SourceEditorProps<FeedSource>): JSX.Element {
  const { t } = useTranslation();
  const preset = feedPresetRoles(value.preset);
  const choices = useAttributeChoices(project, model, value.type);
  const missing = unmappedRoles(value);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap gap-4">
        <label className="flex min-w-48 flex-1 flex-col gap-1 text-caption font-semibold">
          {t("endpoints.ical.type")}
          {frozen ? (
            <span className="font-mono text-body font-normal">{value.type}</span>
          ) : (
            <TypePicker
              id={`${id}-type`}
              label={t("endpoints.ical.type")}
              labelled
              aria-describedby={`${id}-hint`}
              project={project}
              space={space || undefined}
              value={value.type ? [value.type] : []}
              onChange={(types) => onChange({ ...value, type: types[0] ?? value.type })}
            />
          )}
        </label>
        <label className="flex min-w-48 flex-1 flex-col gap-1 text-caption font-semibold">
          {t("endpoints.ical.start")}
          {frozen ? (
            <span className="text-body font-normal">{value.preset ? t(`endpoints.outputs.feedPreset.${value.preset}`) : t("endpoints.ical.presetOwn")}</span>
          ) : (
            <Select
              id={`${id}-preset`}
              aria-describedby={`${id}-hint`}
              value={value.preset ?? ""}
              onChange={(event) => {
                const next: FeedSource = { ...value, preset: event.currentTarget.value || undefined };
                if (!next.preset) delete next.preset;
                onChange(next);
              }}
            >
              {FEED_PRESETS.map((name) => (
                <option key={name} value={name}>
                  {t(`endpoints.outputs.feedPreset.${name}`)}
                </option>
              ))}
              <option value="">{t("endpoints.ical.presetOwn")}</option>
            </Select>
          )}
        </label>
      </div>
      <p id={`${id}-hint`} className="text-caption text-fg-muted">
        {t("endpoints.outputs.type.hint")} {t("endpoints.outputs.feedPresetHint")}
      </p>
      {missing.length > 0 ? <InlineError>{t("endpoints.ical.missing", { fields: missing.join(", ") })}</InlineError> : null}
      <Table stack caption={t("endpoints.outputs.rolesCaption")} zebra={false}>
        <TableHead>
          <TableHeaderCell>{t("endpoints.outputs.role")}</TableHeaderCell>
          <TableHeaderCell>{t("endpoints.ical.attribute")}</TableHeaderCell>
        </TableHead>
        <TableBody>
          {FEED_ROLES.map(({ role, required }) => {
            const picked = value.mapping?.[role];
            const inputId = `${id}-${role}`;
            const fromPreset = preset[role]?.join(" | ");
            return (
              <TableRow key={role}>
                <TableRowHeaderCell>
                  <span className="flex flex-col items-start gap-1">
                    <label htmlFor={inputId}>{t(`endpoints.outputs.roles.${role}`)}</label>
                    {required ? <Badge tone="neutral">{t("endpoints.ical.requiredBadge")}</Badge> : null}
                  </span>
                </TableRowHeaderCell>
                <TableCell className="w-full">
                  {Array.isArray(picked) ? (
                    <span className="font-mono text-caption" title={t("endpoints.ical.remap")}>
                      {picked.join(" | ")}
                    </span>
                  ) : frozen ? (
                    <span className="font-mono text-caption">{picked ?? fromPreset ?? t("endpoints.ical.unmapped")}</span>
                  ) : (
                    <AttributeSelect
                      id={inputId}
                      aria-describedby={`${id}-hint`}
                      value={picked ?? ""}
                      choices={choices}
                      empty={fromPreset ? t("endpoints.outputs.fromPreset", { attribute: fromPreset }) : t("endpoints.ical.unmapped")}
                      invalid={missing.includes(role)}
                      onChange={(attribute) => onChange(withRole(value, role, attribute))}
                    />
                  )}
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
}

/**
 * The map defaults of one class for the catalogue widget (API/11 EP-163): the GeoProperty it draws,
 * the enum or boolean field its markers are coloured by, the fields its popup shows and whether
 * markers cluster. Each dropdown offers only the slots of the type that fit the role, from the
 * model the server lists to this person; the save checks each again and names the one it refuses.
 */
export function MapSourceEditor({ id, value, onChange, project, space, model, frozen }: SourceEditorProps<MapSource>): JSX.Element {
  const { t } = useTranslation();
  const slots = filterSlotsOf(useModelSource(project, model), value.type);
  const geometries = [...new Set([...slots.filter((s) => s.kind === "GeoProperty").map((s) => s.name), ...CORE_GEOMETRIES])];
  const colours = slots.filter((s) => s.kind === "Property" && (s.range === "boolean" || (s.values?.length ?? 0) > 0)).map((s) => s.name);
  const shown = slots.filter((s) => s.kind !== "GeoProperty").map((s) => s.name);
  const popup = value.mapping?.popup ?? [];
  const full = popup.length >= MAX_POPUP;
  const cluster = value.cluster === undefined ? "" : value.cluster ? "on" : "off";

  return (
    <div className="flex flex-col gap-4">
      <label className="flex min-w-48 flex-col gap-1 text-caption font-semibold">
        {t("endpoints.ical.type")}
        {frozen ? (
          <span className="font-mono text-body font-normal">{value.type}</span>
        ) : (
          <TypePicker
            id={`${id}-type`}
            label={t("endpoints.ical.type")}
            labelled
            aria-describedby={`${id}-type-hint`}
            project={project}
            space={space || undefined}
            value={value.type ? [value.type] : []}
            onChange={(types) => onChange({ type: types[0] ?? value.type })}
          />
        )}
        <span id={`${id}-type-hint`} className="font-normal text-fg-muted">
          {t("endpoints.outputs.type.hint")}
        </span>
      </label>
      <div className="grid gap-4 sm:grid-cols-3">
        <Field id={`${id}-geometry`} label={t("endpoints.outputs.map.geometry.label")} help={t("endpoints.outputs.map.geometry.hint")}>
          <AttributeSelect
            id={`${id}-geometry`}
            value={value.mapping?.geometry ?? ""}
            choices={geometries}
            empty={t("endpoints.outputs.map.geometry.default")}
            onChange={(attribute) => onChange(withMapRole(value, "geometry", attribute))}
          />
        </Field>
        <Field id={`${id}-colour`} label={t("endpoints.outputs.map.colour.label")} help={t(colours.length > 0 ? "endpoints.outputs.map.colour.hint" : "endpoints.outputs.map.colour.none")}>
          <AttributeSelect
            id={`${id}-colour`}
            value={value.mapping?.colour ?? ""}
            choices={colours}
            empty={t("endpoints.outputs.map.colour.default")}
            onChange={(attribute) => onChange(withMapRole(value, "colour", attribute))}
          />
        </Field>
        <Field id={`${id}-cluster`} label={t("endpoints.outputs.map.cluster.label")} help={t("endpoints.outputs.map.cluster.hint")}>
          <Select
            id={`${id}-cluster`}
            value={cluster}
            onChange={(event) => {
              const picked = event.currentTarget.value;
              const next: MapSource = { ...value, cluster: picked === "on" };
              if (picked === "") delete next.cluster;
              onChange(next);
            }}
          >
            <option value="">{t("endpoints.outputs.map.cluster.auto")}</option>
            <option value="on">{t("endpoints.outputs.map.cluster.on")}</option>
            <option value="off">{t("endpoints.outputs.map.cluster.off")}</option>
          </Select>
        </Field>
      </div>
      <fieldset className="flex flex-col gap-2" aria-describedby={`${id}-popup-hint`}>
        <legend className="text-caption font-semibold">{t("endpoints.outputs.map.popup.label")}</legend>
        <p id={`${id}-popup-hint`} className="text-caption text-fg-muted">
          {t(shown.length > 0 ? "endpoints.outputs.map.popup.hint" : "endpoints.outputs.map.popup.none", { max: MAX_POPUP })}
        </p>
        <div className="flex flex-wrap gap-x-4 gap-y-1">
          {[...new Set([...shown, ...popup])].map((name) => (
            <Checkbox
              key={name}
              id={`${id}-popup-${name}`}
              label={<span className="font-mono">{name}</span>}
              checked={popup.includes(name)}
              disabled={frozen || (full && !popup.includes(name))}
              disabledReason={full && !popup.includes(name) ? t("endpoints.outputs.map.popup.full", { max: MAX_POPUP }) : undefined}
              onChange={(event) => onChange(withPopup(value, name, event.currentTarget.checked, shown))}
            />
          ))}
        </div>
      </fieldset>
      <TableStartEditor id={id} value={value} onChange={onChange} shown={shown} frozen={frozen} />
    </div>
  );
}

/**
 * Where the widget's table of one class starts (API/11 EP-164): its default filter, order, page
 * length and columns, above the model's `view_*`. Attributes come from the class's slots; the save
 * checks the filter with the gateway's `q` parser and each attribute against the model again.
 */
function TableStartEditor({
  id,
  value,
  onChange,
  shown,
  frozen,
}: {
  id: string;
  value: MapSource;
  onChange: (next: MapSource) => void;
  shown: string[];
  frozen?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const table = value.table ?? {};
  const [orderAttr, orderDir] = (table.orderBy ?? "").split(";");
  const columns = table.columns ?? [];
  const full = columns.length >= MAX_COLUMNS;
  const toggle = (name: string, on: boolean) => {
    const now = new Set(columns);
    if (on) now.add(name);
    else now.delete(name);
    // Ticked in the order the class lists them, then any the class no longer has.
    onChange(withTable(value, "columns", [...shown.filter((n) => now.has(n)), ...[...now].filter((n) => !shown.includes(n))]));
  };
  return (
    <fieldset className="flex flex-col gap-3" aria-describedby={`${id}-table-hint`}>
      <legend className="text-caption font-semibold">{t("endpoints.outputs.table.title")}</legend>
      <p id={`${id}-table-hint`} className="text-caption text-fg-muted">
        {t("endpoints.outputs.table.lead")}
      </p>
      <Field id={`${id}-table-filter`} label={t("endpoints.outputs.table.filter.label")} help={t("endpoints.outputs.table.filter.hint")}>
        <Input
          id={`${id}-table-filter`}
          className="font-mono"
          value={table.filter ?? ""}
          maxLength={1000}
          onChange={(event) => onChange(withTable(value, "filter", event.currentTarget.value.trim() === "" ? undefined : event.currentTarget.value))}
        />
      </Field>
      <div className="grid gap-4 sm:grid-cols-3">
        <Field id={`${id}-table-order`} label={t("endpoints.outputs.table.order.label")} help={t("endpoints.outputs.table.order.hint")}>
          <AttributeSelect
            id={`${id}-table-order`}
            value={orderAttr ?? ""}
            choices={shown}
            empty={t("endpoints.outputs.table.order.default")}
            onChange={(attribute) => onChange(withTable(value, "orderBy", attribute ? `${attribute}${orderDir === "desc" ? ";desc" : ""}` : undefined))}
          />
        </Field>
        <Field id={`${id}-table-direction`} label={t("endpoints.outputs.table.direction.label")} help={t("endpoints.outputs.table.direction.hint")}>
          <Select
            id={`${id}-table-direction`}
            value={orderDir === "desc" ? "desc" : "asc"}
            disabled={!orderAttr}
            title={orderAttr ? undefined : t("endpoints.outputs.table.direction.needsOrder")}
            onChange={(event) => onChange(withTable(value, "orderBy", `${orderAttr}${event.currentTarget.value === "desc" ? ";desc" : ""}`))}
          >
            <option value="asc">{t("endpoints.outputs.table.direction.asc")}</option>
            <option value="desc">{t("endpoints.outputs.table.direction.desc")}</option>
          </Select>
        </Field>
        <Field id={`${id}-table-length`} label={t("endpoints.outputs.table.length.label")} help={t("endpoints.outputs.table.length.hint")}>
          <Select
            id={`${id}-table-length`}
            value={table.pageLength === undefined ? "" : String(table.pageLength)}
            onChange={(event) => onChange(withTable(value, "pageLength", event.currentTarget.value === "" ? undefined : Number(event.currentTarget.value)))}
          >
            <option value="">{t("endpoints.outputs.table.length.default")}</option>
            {TABLE_PAGE_LENGTHS.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </Select>
        </Field>
      </div>
      <fieldset className="flex flex-col gap-2" aria-describedby={`${id}-table-columns-hint`}>
        <legend className="text-caption font-semibold">{t("endpoints.outputs.table.columns.label")}</legend>
        <p id={`${id}-table-columns-hint`} className="text-caption text-fg-muted">
          {t(shown.length > 0 ? "endpoints.outputs.table.columns.hint" : "endpoints.outputs.table.columns.none", { max: MAX_COLUMNS })}
        </p>
        <div className="flex flex-wrap gap-x-4 gap-y-1">
          {[...new Set([...shown, ...columns])].map((name) => (
            <Checkbox
              key={name}
              id={`${id}-table-column-${name}`}
              label={<span className="font-mono">{name}</span>}
              checked={columns.includes(name)}
              disabled={frozen || (full && !columns.includes(name))}
              disabledReason={full && !columns.includes(name) ? t("endpoints.outputs.table.columns.full", { max: MAX_COLUMNS }) : undefined}
              onChange={(event) => toggle(name, event.currentTarget.checked)}
            />
          ))}
        </div>
      </fieldset>
    </fieldset>
  );
}
