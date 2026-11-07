import { useId, useState } from "react";
import type { HTMLAttributes, JSX } from "react";
import { clsx } from "clsx";
import { useTranslation } from "react-i18next";
import { Button } from "./Button";
import { ExternalLink } from "./ExternalLink";
import { Icon } from "./icons";
import { cellOf, isRecord, recordJson } from "./recordView";
import type { RecordCell } from "./recordView";

/** A string longer than this shows its start and a "Show all" (T-5516). */
const LONG = 160;

export interface RecordViewProps extends Omit<HTMLAttributes<HTMLDivElement>, "tabIndex" | "children"> {
  /** The record: an NGSI-LD entity, a pipeline message, a list of them or plain text. */
  value: unknown;
  /** What the record is ("Sample record"), read when the view takes focus. */
  label: string;
}

/**
 * The Portal's one view of a data record (T-5516): label/value rows with each NGSI-LD attribute
 * unwrapped to what it holds, a geometry as coordinates, nested objects collapsible and long text
 * cut short, and "Copy as JSON" for the raw form. Text only, links through `safeHref`. Like
 * `CodeBlock` the view may scroll, so it is a named tab stop (WCAG 2.1.1).
 */
export function RecordView({ value, label, className, ...rest }: RecordViewProps): JSX.Element {
  return (
    <div role="group" aria-label={label} tabIndex={0} className={clsx("focus-ring overflow-auto text-caption", className)} {...rest}>
      <div className="mb-1 flex justify-end">
        <CopyJson value={value} />
      </div>
      <Body value={value} depth={0} />
    </div>
  );
}

function CopyJson({ value }: { value: unknown }): JSX.Element {
  const { t } = useTranslation();
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const copy = () => {
    const writing = navigator.clipboard?.writeText(recordJson(value));
    if (!writing) return setState("failed");
    void writing.then(() => setState("copied")).catch(() => setState("failed"));
  };
  return (
    <span className="flex items-center gap-2">
      {state === "failed" ? (
        <span role="alert" className="text-fg-muted">
          {t("recordView.copyFailed")}
        </span>
      ) : null}
      <Button size="sm" variant="ghost" icon={<Icon name={state === "copied" ? "check" : "copy"} className="size-4" />} onClick={copy}>
        {state === "copied" ? t("recordView.copied") : t("recordView.copy")}
      </Button>
    </span>
  );
}

function Body({ value, depth }: { value: unknown; depth: number }): JSX.Element {
  const { t } = useTranslation();
  if (typeof value === "string") return <pre className="whitespace-pre-wrap break-words font-mono">{value}</pre>;
  if (Array.isArray(value)) {
    if (value.length === 0) return <p className="text-fg-muted">{t("recordView.noItems")}</p>;
    return (
      <ol className="flex flex-col gap-2">
        {value.map((item, at) => (
          <li key={at} className="flex flex-col gap-1 border-l-2 border-border pl-2">
            <span className="font-medium text-fg-muted">{t("recordView.item", { n: at + 1 })}</span>
            <Body value={item} depth={depth + 1} />
          </li>
        ))}
      </ol>
    );
  }
  if (!isRecord(value)) return <Value cell={cellOf(value)} depth={depth} />;
  const entries = Object.entries(value);
  if (entries.length === 0) return <p className="text-fg-muted">{t("recordView.noFields")}</p>;
  return (
    <dl className="grid grid-cols-[minmax(0,max-content)_minmax(0,1fr)] gap-x-3 gap-y-1">
      {entries.map(([key, member]) => (
        <div key={key} className="contents">
          <dt className="break-words font-medium text-fg-muted">{key}</dt>
          <dd className="min-w-0 break-words text-fg">
            <Value cell={cellOf(member)} depth={depth} />
          </dd>
        </div>
      ))}
    </dl>
  );
}

function Value({ cell, depth }: { cell: RecordCell; depth: number }): JSX.Element {
  return (
    <div className="flex flex-col gap-0.5">
      {cell.kind === "nested" ? <Nested value={cell.value} depth={depth + 1} /> : <Text text={cell.text} href={cell.href} />}
      {cell.unit !== undefined || cell.at !== undefined ? (
        <span className="text-fg-subtle">
          {[cell.unit, cell.at].filter((part) => part !== undefined).join(" · ")}
        </span>
      ) : null}
    </div>
  );
}

function Text({ text, href }: { text: string; href?: string }): JSX.Element {
  const { t } = useTranslation();
  const [all, setAll] = useState(false);
  if (href !== undefined && text.length <= LONG) return <ExternalLink href={href}>{text}</ExternalLink>;
  if (text.length <= LONG) return <span>{text}</span>;
  return (
    <span>
      {all ? text : `${text.slice(0, LONG)}…`}{" "}
      <Button size="sm" variant="ghost" aria-expanded={all} onClick={() => setAll(!all)}>
        {all ? t("recordView.showLess") : t("recordView.showAll")}
      </Button>
    </span>
  );
}

/** A nested object or list: open on the first level, folded below it. */
export function Nested({ value, depth }: { value: Record<string, unknown> | unknown[]; depth: number }): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const [open, setOpen] = useState(depth < 2);
  const count = Array.isArray(value) ? value.length : Object.keys(value).length;
  return (
    <div className="flex flex-col gap-1">
      <div>
        <Button
          size="sm"
          variant="ghost"
          aria-expanded={open}
          aria-controls={id}
          icon={<Icon name={open ? "chevronDown" : "chevronRight"} className="size-4" />}
          onClick={() => setOpen(!open)}
        >
          {t(Array.isArray(value) ? "recordView.items" : "recordView.fields", { count })}
        </Button>
      </div>
      <div id={id} hidden={!open} className="border-l-2 border-border pl-2">
        {open ? <Body value={value} depth={depth} /> : null}
      </div>
    </div>
  );
}
