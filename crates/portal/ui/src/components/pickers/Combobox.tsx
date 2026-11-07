import { useEffect, useEffectEvent, useId, useMemo, useRef, useState } from "react";
import type { JSX, KeyboardEvent, ReactNode } from "react";
import { clsx } from "clsx";
import { useTranslation } from "react-i18next";
import { Badge, Button, InlineError, Input } from "../ui";
import { Icon } from "../ui/icons";

/** One thing a picker offers: its value, what a person reads, and the group it sits under. */
export interface PickerOption {
  value: string;
  label: string;
  /** A second line: the owner, the name under the title. */
  detail?: string;
  /** The heading the option is listed under (a project and space, "Smart Data Models"). */
  group: string;
  /** A short tag at the end of the row: the version, the lifecycle. */
  badge?: string;
}

export interface ComboboxProps {
  id?: string;
  /** The accessible name of the search box and the list. */
  label: string;
  /** A `<label for={id}>` of the form names the search box, so it carries no `aria-label` of its own. */
  labelled?: boolean;
  /** The chosen values: one for a single picker, any number for `multiple`. */
  value: string[];
  onChange: (values: string[]) => void;
  options: PickerOption[];
  multiple?: boolean;
  disabled?: boolean;
  required?: boolean;
  invalid?: boolean;
  describedBy?: string;
  /** What a wrapping `Field` stamps on its one child: its description, help and error ids. */
  "aria-describedby"?: string;
  "aria-invalid"?: boolean | "true" | "false";
  placeholder?: string;
  loading?: boolean;
  /** Why the list could not be read, with a retry; a failed list is not an empty one (T-1486). */
  failed?: { reason: string; retry: () => void };
  /** What the list says when nothing matches. */
  empty: string;
  /** Typed text that a server search should also answer (debounced by the caller's query). */
  onSearch?: (text: string) => void;
  /**
   * The caller answers `onSearch` with `options` already matched and ranked (the unit list
   * matches `ug/m3` to `µg/m³`), so the picker lists them as given instead of filtering again.
   */
  searched?: boolean;
  /** Offered under the list: a person names something that does not exist yet. */
  create?: { label: (text: string) => string; onCreate: (text: string) => void };
  /** Shown under the list, e.g. why a catalogue could not be searched. */
  note?: ReactNode;
}

/**
 * The one searchable picker every form uses for a name of something that exists (ADR-N-033): a
 * combobox over a grouped listbox, operated by pointer or keyboard (arrows, Home, End, Enter,
 * Escape), with the chosen values as removable pills above it when it takes several.
 */
export function Combobox({
  id,
  label,
  labelled = false,
  value,
  onChange,
  options,
  multiple = false,
  disabled = false,
  required = false,
  invalid = false,
  describedBy,
  "aria-describedby": fieldDescribedBy,
  "aria-invalid": fieldInvalid,
  placeholder,
  loading = false,
  failed,
  empty,
  onSearch,
  searched = false,
  create,
  note,
}: ComboboxProps): JSX.Element {
  const { t } = useTranslation();
  const generated = useId();
  const inputId = id ?? generated;
  const listId = `${inputId}-list`;
  const root = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState<string | null>(null);
  const [active, setActive] = useState(0);

  const labelOf = (v: string) => options.find((o) => o.value === v)?.label ?? v;
  const words = (typed ?? "").trim().toLowerCase();
  const shown = useMemo(
    () =>
      options.filter(
        (o) =>
          searched ||
          words === "" ||
          o.label.toLowerCase().includes(words) ||
          o.value.toLowerCase().includes(words) ||
          (o.detail ?? "").toLowerCase().includes(words) ||
          o.group.toLowerCase().includes(words),
      ),
    [options, words, searched],
  );
  const canCreate = Boolean(create && words !== "" && !options.some((o) => o.value.toLowerCase() === words));
  const count = shown.length + (canCreate ? 1 : 0);


  const pick = (v: string) => {
    if (multiple) {
      onChange(value.includes(v) ? value.filter((x) => x !== v) : [...value, v]);
      setTyped("");
      onSearch?.("");
    } else {
      onChange([v]);
      setOpen(false);
      setTyped(null);
    }
  };

  /**
   * The person left the picker. Where the form may name something new, what was typed is the
   * answer, as in a text field; elsewhere it is dropped and the chosen value shows again.
   */
  const leave = () => {
    const text = (typed ?? "").trim();
    if (create && text !== "" && !value.includes(text)) {
      const known = options.find((o) => o.value.toLowerCase() === text.toLowerCase());
      if (known) {
        pick(known.value);
      } else {
        create.onCreate(text);
      }
    }
    setOpen(false);
    setTyped(null);
  };

  const leaveOutside = useEffectEvent(() => {
    leave();
  });

  useEffect(() => {
    if (!open) {
      return;
    }
    const onDown = (event: MouseEvent) => {
      if (root.current && !root.current.contains(event.target as Node)) {
        leaveOutside();
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => {
      document.removeEventListener("mousedown", onDown);
    };
  }, [open]);


  const enter = (text: string) => {
    setTyped(text);
    setActive(0);
    setOpen(true);
    onSearch?.(text);
  };

  const choose = (index: number) => {
    if (index < shown.length) {
      pick(shown[index].value);
    } else if (canCreate && create) {
      create.onCreate((typed ?? "").trim());
      setOpen(false);
      setTyped(null);
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (!open) {
        setOpen(true);
        setActive(0);
      } else {
        setActive((at) => Math.min(at + 1, Math.max(count - 1, 0)));
      }
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((at) => Math.max(at - 1, 0));
    } else if (event.key === "Home" && open) {
      event.preventDefault();
      setActive(0);
    } else if (event.key === "End" && open) {
      event.preventDefault();
      setActive(Math.max(count - 1, 0));
    } else if (event.key === "Enter" && open) {
      event.preventDefault();
      choose(active);
    } else if (event.key === "Escape" && open) {
      event.preventDefault();
      setOpen(false);
      setTyped(null);
    } else if (event.key === "Backspace" && multiple && (typed ?? "") === "" && value.length > 0) {
      onChange(value.slice(0, -1));
    }
  };

  const optionId = (index: number) => `${listId}-${index}`;
  const shownText = typed ?? (multiple || value.length === 0 ? "" : labelOf(value[0]));

  return (
    <div ref={root} className="relative flex flex-col gap-1">
      {multiple && value.length > 0 ? (
        <ul aria-label={t("picker.chosen", { label })} className="flex flex-wrap gap-1">
          {value.map((v) => (
            <li
              key={v}
              className="inline-flex items-center gap-1 rounded-full border border-primary-200 bg-primary-soft py-0.5 pl-2 pr-1 text-caption text-primary-soft-fg"
            >
              <span className="max-w-48 truncate">{labelOf(v)}</span>
              <Button
                variant="ghost"
                size="xs"
                disabled={disabled}
                aria-label={t("picker.remove", { name: labelOf(v) })}
                onClick={() => {
                  onChange(value.filter((x) => x !== v));
                }}
                className="rounded-full px-1"
              >
                ×
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
      <div className="relative">
        <Icon
          name="search"
          aria-hidden="true"
          className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-fg-muted"
        />
        <Input
          id={inputId}
          role="combobox"
          aria-label={labelled ? undefined : label}
          aria-expanded={open}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={open && count > 0 ? optionId(active) : undefined}
          aria-describedby={[describedBy, fieldDescribedBy].filter(Boolean).join(" ") || undefined}
          aria-invalid={invalid || fieldInvalid === true || fieldInvalid === "true" ? "true" : undefined}
          aria-required={required || undefined}
          disabled={disabled}
          placeholder={placeholder ?? t("picker.search")}
          autoComplete="off"
          value={shownText}
          onFocus={() => {
            setOpen(true);
          }}
          onClick={() => {
            setOpen(true);
          }}
          onBlur={(event) => {
            // Focus moving to the list's own retry button is not leaving.
            if (!root.current?.contains(event.relatedTarget as Node | null)) {
              leave();
            }
          }}
          onChange={(event) => {
            enter(event.target.value);
          }}
          onPaste={(event) => {
            // A one-line field drops line breaks, which runs a pasted column of names into one
            // word; where the form may name something new, each line becomes a comma (T-5605).
            const text = event.clipboardData.getData("text");
            if (!create || !/[\r\n]/.test(text)) {
              return;
            }
            event.preventDefault();
            const input = event.currentTarget;
            const lines = text.trim().split(/\s*[\r\n]+\s*/).join(", ");
            enter(input.value.slice(0, input.selectionStart ?? input.value.length) + lines + input.value.slice(input.selectionEnd ?? input.value.length));
          }}
          onKeyDown={onKeyDown}
          className="pl-7"
        />
      </div>
      {open ? (
        <div className="absolute left-0 top-full z-50 mt-1 w-full min-w-64 rounded-lg border border-border bg-surface p-1 shadow-2">
          {loading ? <p className="px-2 py-1 text-caption text-fg-muted">{t("app.loading")}</p> : null}
          {failed ? (
            <InlineError className="px-2 py-1">
              {t("form.listFailed", { reason: failed.reason })}{" "}
              <Button
                variant="ghost"
                size="xs"
                className="px-0 text-caption text-danger underline hover:no-underline"
                onClick={failed.retry}
              >
                {t("form.listRetry")}
              </Button>
            </InlineError>
          ) : null}
          {!loading && !failed && shown.length === 0 && !canCreate ? (
            <p className="px-2 py-1 text-caption text-fg-muted">{empty}</p>
          ) : null}
          <ul
            id={listId}
            role="listbox"
            aria-label={labelled ? t("picker.list") : label}
            aria-multiselectable={multiple || undefined}
            className="max-h-64 overflow-y-auto"
          >
            {shown.map((option, index) => {
              const heading = index === 0 || shown[index - 1].group !== option.group ? option.group : undefined;
              const selected = value.includes(option.value);
              return (
                <li
                  key={`${option.group}\u0000${option.value}`}
                  id={optionId(index)}
                  role="option"
                  aria-selected={selected}
                  onMouseEnter={() => {
                    setActive(index);
                  }}
                  onMouseDown={(event) => {
                    event.preventDefault();
                  }}
                  onClick={() => {
                    choose(index);
                  }}
                  className="cursor-pointer"
                >
                  {heading !== undefined ? (
                    <span aria-hidden="true" className="block px-2 pb-0.5 pt-2 text-caption font-medium text-fg-muted">
                      {heading}
                    </span>
                  ) : null}
                  <span
                    className={clsx(
                      "flex items-center gap-2 rounded-md px-2 py-1.5",
                      index === active && "bg-surface-subtle",
                    )}
                  >
                    <span className="flex size-4 shrink-0 items-center justify-center text-primary-soft-fg">
                      {selected ? <Icon name="check" className="size-3.5" /> : null}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate font-medium text-fg">{option.label}</span>
                      {option.detail ? (
                        <span className="block truncate text-caption text-fg-muted">{option.detail}</span>
                      ) : null}
                    </span>
                    {option.badge ? (
                      <Badge tone="neutral" className="shrink-0">
                        {option.badge}
                      </Badge>
                    ) : null}
                  </span>
                </li>
              );
            })}
            {canCreate && create ? (
              <li
                id={optionId(shown.length)}
                role="option"
                aria-selected={false}
                onMouseEnter={() => {
                  setActive(shown.length);
                }}
                onMouseDown={(event) => {
                  event.preventDefault();
                }}
                onClick={() => {
                  choose(shown.length);
                }}
                className={clsx(
                  "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-primary-soft-fg",
                  active === shown.length && "bg-surface-subtle",
                )}
              >
                <Icon name="plus" className="size-3.5" />
                {create.label((typed ?? "").trim())}
              </li>
            ) : null}
          </ul>
          {note ? <div className="px-2 py-1 text-caption text-fg-muted">{note}</div> : null}
        </div>
      ) : null}
    </div>
  );
}
