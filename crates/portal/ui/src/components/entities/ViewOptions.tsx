/**
 * What a view shows beyond its filter and order (API/01 §30, T-3099): the attributes it hides, the
 * colour rules that mark rows, and the enum attribute it groups by, with each group's count read
 * from the space itself.
 */
import { useId } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { useQueries } from "@tanstack/react-query";
import { andQ, parseQ } from "@joinedcontext/sdk";
import type { EntitySource, EnumOption } from "@joinedcontext/sdk";
import type { ViewConfig } from "../../api/dataViews";
import { Button, Checkbox, Field, Input, Select } from "../ui";

export const TONES = ["neutral", "info", "success", "warning", "danger"] as const;
export type ViewExtras = Pick<ViewConfig, "hidden" | "colour" | "group" | "width" | "order" | "labels">;
type ColourRule = NonNullable<ViewConfig["colour"]>[number];

/** Groups counted at most: an enum longer than this is a list to filter, not to count. */
export const MAX_GROUPS = 50;

export interface ViewOptionsProps {
  /** Every attribute the type has, as the grid and the model know them. */
  attributes: string[];
  /** The enum attributes and their values: what a view can group by. */
  enums: Record<string, EnumOption[]>;
  value: ViewExtras;
  onChange: (next: ViewExtras) => void;
  /** One part alone, as the grid's toolbar opens it in its own panel (T-4883); all three without it. */
  part?: "fields" | "colours" | "group";
  /** A field's title as its column shows it; the attribute's name without it. */
  labelOf?: (attr: string) => string;
}

export function ViewOptions({ attributes, enums, value, onChange, part, labelOf }: ViewOptionsProps): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const hidden = new Set(value.hidden ?? []);
  const colour = value.colour ?? [];
  const setColour = (next: ColourRule[]) => onChange({ ...value, colour: next });

  const fields = (
        <fieldset className="flex flex-col gap-1">
          <legend className="text-body font-semibold">{t("spaces.saved.fields")}</legend>
          {attributes.length === 0 ? <p className="text-caption text-fg-muted">{t("spaces.saved.noFields")}</p> : null}
          {attributes.map((attr) => (
            <Checkbox
              key={attr}
              label={labelOf?.(attr) ?? attr}
              checked={!hidden.has(attr)}
              onChange={(e) => {
                const next = new Set(hidden);
                if (e.target.checked) next.delete(attr);
                else next.add(attr);
                onChange({ ...value, hidden: [...next].sort() });
              }}
            />
          ))}
        </fieldset>
  );
  const colours = (
        <fieldset className="flex flex-col gap-2">
          <legend className="text-body font-semibold">{t("spaces.saved.colours")}</legend>
          <p className="text-caption text-fg-muted">{t("spaces.saved.coloursHelp")}</p>
          {Object.keys(enums).length > 0 ? (
            <Field id={`${id}-by-enum`} label={t("spaces.saved.colourBy")} help={t("spaces.saved.colourByHelp")}>
              <Select
                id={`${id}-by-enum`}
                value=""
                onChange={(e) => {
                  if (e.target.value) setColour(enumColours(e.target.value, enums[e.target.value] ?? []));
                }}
              >
                <option value="">{t("spaces.saved.colourByPick")}</option>
                {Object.keys(enums).map((attr) => (
                  <option key={attr} value={attr}>
                    {attr}
                  </option>
                ))}
              </Select>
            </Field>
          ) : null}
          {colour.map((rule, index) => {
            const unreadable = rule.when.trim() !== "" && parseQ(rule.when) === null;
            return (
              <div key={index} className="flex flex-col gap-1 rounded-md border border-border p-2">
                <Field
                  id={`${id}-when-${index}`}
                  label={t("spaces.saved.when")}
                  errors={unreadable ? [t("spaces.saved.whenUnreadable")] : undefined}
                >
                  <Input
                    id={`${id}-when-${index}`}
                    value={rule.when}
                    placeholder="availableBikeNumber==0"
                    onChange={(e) => setColour(colour.map((r, at) => (at === index ? { ...r, when: e.target.value } : r)))}
                  />
                </Field>
                <Field id={`${id}-tone-${index}`} label={t("spaces.saved.tone")}>
                  <Select
                    id={`${id}-tone-${index}`}
                    value={rule.colour}
                    onChange={(e) => setColour(colour.map((r, at) => (at === index ? { ...r, colour: e.target.value } : r)))}
                  >
                    {TONES.map((tone) => (
                      <option key={tone} value={tone}>
                        {t(`spaces.saved.tones.${tone}`)}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Button size="sm" variant="ghost" className="w-fit" onClick={() => setColour(colour.filter((_, at) => at !== index))}>
                  {t("spaces.saved.removeRule", { n: index + 1 })}
                </Button>
              </div>
            );
          })}
          <Button
            size="sm"
            className="w-fit"
            disabled={colour.length >= 20}
          disabledReason={t("spaces.saved.coloursFull")}
            onClick={() => setColour([...colour, { when: "", colour: "warning" }])}
          >
            {t("spaces.saved.addRule")}
          </Button>
        </fieldset>
  );
  const grouping = (
        <Field id={`${id}-group`} label={t("spaces.saved.group")} help={t("spaces.saved.groupHelp")}>
          <Select
            id={`${id}-group`}
            value={value.group ?? ""}
            onChange={(e) => onChange({ ...value, group: e.target.value || undefined })}
          >
            <option value="">{t("spaces.saved.noGroup")}</option>
            {Object.keys(enums).map((attr) => (
              <option key={attr} value={attr}>
                {attr}
              </option>
            ))}
          </Select>
        </Field>
  );
  if (part) return { fields, colours, group: grouping }[part];
  return (
    <details className="rounded-md border border-border p-3">
      <summary className="cursor-pointer text-body font-semibold">{t("spaces.saved.options")}</summary>
      <div className="mt-3 grid gap-4 md:grid-cols-3">
        {fields}
        {colours}
        {grouping}
      </div>
    </details>
  );
}

/** Rules that colour rows by an enum's value (T-5186): one per value, at most twenty, the tones in turn. */
export function enumColours(attr: string, options: EnumOption[]): ColourRule[] {
  const tones = TONES.filter((tone) => tone !== "neutral");
  return options.slice(0, 20).map((option, at) => ({ when: groupTerm(attr, option.value), colour: tones[at % tones.length] }));
}

/** The query that narrows the grid to one group: the enum attribute equal to one value. */
export function groupTerm(attr: string, value: string): string {
  return `${attr}==${JSON.stringify(value)}`;
}

export interface GroupCountsProps {
  /** Whose counts these are (`{project}/{space}`): every count's cache key names it (F-UI1-006). */
  scope: string;
  source: EntitySource;
  type: string;
  attr: string;
  options: EnumOption[];
  /** The view's own query, which every count is narrowed by. */
  q?: string;
  /** The group the grid is narrowed to, if any. */
  chosen: string | null;
  onChoose: (value: string | null) => void;
}

/**
 * One count per value of the grouping attribute, each the space's own `count=true` answer read
 * with the person's session: the counts narrow as their grants do and are never the page's tally.
 */
export function GroupCounts({ scope, source, type, attr, options, q, chosen, onChoose }: GroupCountsProps): JSX.Element {
  const { t } = useTranslation();
  const shown = options.slice(0, MAX_GROUPS);
  const counts = useQueries({
    queries: shown.map((option) => ({
      queryKey: ["group-count", scope, type, attr, option.value, q ?? ""],
      retry: false,
      staleTime: 30_000,
      queryFn: async () => (await source.query({ type, q: andQ(q, groupTerm(attr, option.value)) }, { offset: 0, limit: 1 })).total ?? null,
    })),
  });
  const countText = (index: number) => {
    const count = counts[index];
    if (count.isPending) return "…";
    if (count.isError || count.data === null || count.data === undefined) return t("spaces.saved.countUnknown");
    return String(count.data);
  };

  return (
    <nav aria-label={t("spaces.saved.groupsOf", { attr })} className="flex flex-col gap-1">
      <ul className="flex flex-wrap gap-2">
        <li>
          <Button size="sm" variant={chosen === null ? "primary" : "secondary"} aria-pressed={chosen === null} onClick={() => onChoose(null)}>
            {t("spaces.saved.allGroups")}
          </Button>
        </li>
        {shown.map((option, index) => (
          <li key={option.value}>
            <Button
              size="sm"
              variant={chosen === option.value ? "primary" : "secondary"}
              aria-pressed={chosen === option.value}
              onClick={() => onChoose(option.value)}
            >
              {`${option.title ?? option.value} (${countText(index)})`}
            </Button>
          </li>
        ))}
      </ul>
      {options.length > MAX_GROUPS ? (
        <p className="text-caption text-fg-muted">{t("spaces.saved.groupsCut", { shown: MAX_GROUPS, total: options.length })}</p>
      ) : null}
    </nav>
  );
}
