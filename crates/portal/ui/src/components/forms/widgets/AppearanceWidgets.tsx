import { clsx } from "clsx";
import type { JSX } from "react";
import type { FieldProps, WidgetProps } from "@rjsf/utils";
import { useTranslation } from "react-i18next";
import { BaseInputTemplate, LIST_BODY, LIST_FIELDSET, ListLegend, TextareaWidget } from "../theme";
import { RECORD_LINK_STYLE } from "../../RecordLink";

const HEX = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;

/**
 * A colour as `#rrggbb` with a swatch beside it (UIX-16, T-6772): the swatch shows the colour only
 * once the value is one, so a refused value never paints anything.
 */
export function ColourWidget(props: WidgetProps): JSX.Element {
  return (
    <span className="flex items-center gap-2">
      <Swatch colour={typeof props.value === "string" ? props.value : undefined} className="h-control w-control" />
      <span className="min-w-0 flex-1">
        <BaseInputTemplate {...props} />
      </span>
    </span>
  );
}

/** A `data:` URL of SVG text for an `<img>`, which never runs what the SVG holds; none for other text. */
export function svgImageUrl(svg: string | undefined): string | undefined {
  const text = svg?.trim() ?? "";
  return text.startsWith("<svg") ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(text)}` : undefined;
}

/** A colour square: the colour, a dashed empty square when unset, a struck-through red square for a value that is no colour. */
export function Swatch({ colour, className }: { colour?: string; className: string }): JSX.Element {
  const text = colour?.trim() ?? "";
  const valid = HEX.test(text);
  const refused = text !== "" && !valid;
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 16 16"
      className={clsx("shrink-0 rounded border", refused ? "border-danger" : text === "" ? "border-dashed border-fg-muted" : "border-border", className)}
    >
      <rect width="16" height="16" className={valid ? undefined : "fill-surface"} fill={valid ? text : undefined} />
      {refused ? <line x1="0" y1="16" x2="16" y2="0" className="stroke-danger" strokeWidth="1.5" /> : null}
    </svg>
  );
}

/**
 * Pasted SVG with a preview under it (UIX-16, T-6772). The preview is an `<img>` of a data URL,
 * never inline markup, so nothing in the pasted text runs in the Portal.
 */
export function SvgImageWidget(props: WidgetProps): JSX.Element {
  const { t } = useTranslation();
  const url = svgImageUrl(typeof props.value === "string" ? props.value : undefined);
  return (
    <span className="flex flex-col gap-2">
      <TextareaWidget {...props} />
      {url ? (
        <span className="flex items-center gap-3 text-caption text-fg-muted">
          <img
            src={url}
            alt={t("form.svgPreview", { label: props.label })}
            className="h-16 max-w-48 rounded border border-border bg-surface object-contain p-1"
          />
          {t("form.svgPreviewNote")}
        </span>
      ) : null}
    </span>
  );
}

/**
 * A list of choices the server offers none of yet (T-6779: a home page's Dashboards): the list's
 * heading over a dashed frame that says how to make one and links to the Organization's projects,
 * instead of an empty list with nothing to add.
 */
export function NoChoicesField(props: FieldProps): JSX.Element {
  const { t } = useTranslation();
  const help = props.uiSchema?.["ui:help"];
  const id = props.fieldPathId.$id;
  return (
    <fieldset id={id} className={LIST_FIELDSET} aria-describedby={`${id}__help`}>
      <ListLegend heading={props.schema.title ?? ""} required={props.required} />
      <div className={clsx(LIST_BODY, "max-w-2xl rounded-md border border-dashed border-fg-muted p-3")}>
        <p id={`${id}__help`} className="text-body text-fg-muted">
          {typeof help === "string" ? help : null}
        </p>
        <a href="/organization/projects" className={clsx(RECORD_LINK_STYLE, "self-start underline")}>
          {t("organization.field.appearance.homeWidgetsProjects")}
        </a>
      </div>
    </fieldset>
  );
}
