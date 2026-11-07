import { Fragment, type ReactNode } from "react";
import { columnKind, displayName, format, pointOf } from "@joinedcontext/sdk";
import type { Cell, Row } from "@joinedcontext/sdk";
import { Empty } from "./states";
import { t } from "../i18n";

export function EntityDetail({
  row,
  attrs,
  title,
  onClose,
}: {
  row: Row | null | undefined;
  attrs?: string[];
  title?: string;
  onClose?: () => void;
}): React.JSX.Element {
  if (!row) {
    return <Empty>{t("detail.none")}</Empty>;
  }

  const resolvedAttrs =
    attrs && attrs.length > 0
      ? attrs
      : [
          ...("id" in row ? ["id"] : []),
          ...("type" in row ? ["type"] : []),
          ...Object.keys(row).filter((k) => k !== "@context" && k !== "id" && k !== "type"),
        ];

  const renderValue = (val: Cell, attr: string): ReactNode => {
    if (attr === "id") {
      return <code>{format(val)}</code>;
    }
    if (val !== null && typeof val === "object") {
      const pt = pointOf(val);
      if (pt && val.type === "Point") {
        return `${pt[1].toFixed(5)}, ${pt[0].toFixed(5)}`;
      }
      if ("type" in val && typeof (val as { type: unknown }).type === "string") {
        return (val as { type: string }).type;
      }
    }
    return format(val, columnKind([row], attr));
  };

  return (
    <section className="jc-detail">
      <header>
        <h2>{title ?? displayName(row)}</h2>
        {onClose && (
          <button type="button" aria-label={t("detail.close")} onClick={onClose}>
            ×
          </button>
        )}
      </header>
      <dl>
        {resolvedAttrs.map((attr) => (
          <Fragment key={attr}>
            <dt>{attr}</dt>
            <dd>{renderValue(row[attr], attr)}</dd>
          </Fragment>
        ))}
      </dl>
    </section>
  );
}
