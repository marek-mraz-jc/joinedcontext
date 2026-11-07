import React, { useEffect, useId, useRef, useState } from "react";
import type { RelationEnd, TargetOption } from "../relations";

/** The strings a relationship picker shows; the grid and the form each pass their own. */
export interface PickerLabels {
  /** Said before the target class on the search box: "Search School". */
  search: string;
  /** Said when the search found nothing the person can read. */
  none: string;
  /** Said before a target on its remove button. */
  remove: string;
  loading: string;
  /** Said when the search itself failed. */
  failed: string;
}

/** The NGSI-LD null: what a cleared end is written as, so the attribute goes (CIM 009 §4.5.0). */
export { NGSI_LD_NULL } from "./model";

/**
 * One stored end of a relationship, edited by picking entities of its target class (UI-84).
 *
 * The cardinality is the control's own: a single end holds one target and a pick replaces it, a
 * many end adds one, and the last target of a required end has no remove button, so the person
 * cannot build a write the model refuses. A search names only entities the source answers, which
 * is what the person may read. A server refusal of this end is shown here, beside the value.
 */
export function RelationPicker({
  label,
  value,
  end,
  search,
  labels,
  changed = false,
  invalid,
  onChange,
}: {
  label: string;
  value: string[];
  end: RelationEnd;
  search: (text: string) => Promise<TargetOption[]>;
  labels: PickerLabels;
  changed?: boolean;
  /** Why the endpoint refused this end, in its own words. */
  invalid?: string;
  onChange: (next: string[]) => void;
}): React.JSX.Element {
  const id = useId();
  const listId = `${id}-list`;
  const errorId = `${id}-error`;
  const [text, setText] = useState("");
  const [open, setOpen] = useState(false);
  const [options, setOptions] = useState<TargetOption[]>([]);
  const [active, setActive] = useState(0);
  const [status, setStatus] = useState<"idle" | "loading" | "failed">("idle");
  // The names the searches brought, so a picked target keeps reading as its name.
  const [names, setNames] = useState<Record<string, string>>({});
  const asked = useRef(0);

  useEffect(() => {
    if (!open) {
      return;
    }
    const ask = ++asked.current;
    setStatus("loading");
    // A pause after the last key: one read per word typed, not one per letter.
    const timer = setTimeout(() => {
      search(text).then(
        (found) => {
          if (ask !== asked.current) return;
          setOptions(found);
          setActive(0);
          setStatus("idle");
          setNames((known) => ({ ...known, ...Object.fromEntries(found.flatMap((one) => (one.name ? [[one.id, one.name]] : []))) }));
        },
        () => {
          if (ask !== asked.current) return;
          setOptions([]);
          setStatus("failed");
        },
      );
    }, 200);
    return () => clearTimeout(timer);
  }, [open, text, search]);

  const offered = options.filter((option) => !value.includes(option.id));
  const pick = (option: TargetOption) => {
    onChange(end.many ? [...value, option.id] : [option.id]);
    setText("");
    setOpen(false);
  };
  // The last target of a required end stays: removing it is the write the model refuses.
  const removable = !(end.required && value.length <= 1);
  const shown = (target: string) => names[target] ?? target;

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!open) {
        setOpen(true);
        return;
      }
      const step = e.key === "ArrowDown" ? 1 : -1;
      setActive((at) => (offered.length === 0 ? 0 : (at + step + offered.length) % offered.length));
    } else if (e.key === "Enter") {
      if (open && offered[active]) {
        e.preventDefault();
        pick(offered[active]);
      }
    } else if (e.key === "Escape" && open) {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    }
  };

  const said =
    status === "loading" ? labels.loading : status === "failed" ? labels.failed : open && offered.length === 0 ? labels.none : "";

  return (
    <div
      className={`jc-rel${changed ? " jc-grid-cell-changed" : ""}${invalid ? " jc-grid-cell-invalid" : ""}`}
      role="group"
      aria-label={label}
      data-changed={changed ? "true" : undefined}
    >
      {value.map((target) => (
        <span key={target} className="jc-rel-chip" title={target}>
          {shown(target)}
          {removable && (
            <button
              type="button"
              className="jc-rel-remove"
              aria-label={`${labels.remove} ${shown(target)}`}
              onClick={() => onChange(value.filter((one) => one !== target))}
            >
              ×
            </button>
          )}
        </span>
      ))}
      <input
        className="jc-grid-cell-input jc-rel-input"
        role="combobox"
        aria-label={`${labels.search} ${end.target}`}
        aria-autocomplete="list"
        aria-expanded={open}
        aria-controls={listId}
        aria-activedescendant={open && offered[active] ? `${listId}-${active}` : undefined}
        aria-required={end.required || undefined}
        aria-invalid={invalid ? true : undefined}
        aria-describedby={invalid ? errorId : undefined}
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onKeyDown={onKeyDown}
      />
      <ul id={listId} role="listbox" aria-label={`${labels.search} ${end.target}`} className="jc-rel-list" hidden={!open || offered.length === 0}>
        {offered.map((option, index) => (
          <li
            key={option.id}
            id={`${listId}-${index}`}
            role="option"
            aria-selected={index === active}
            className="jc-rel-option"
            // Keeps the focus in the box, so the pick lands before the blur closes the list.
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => pick(option)}
          >
            {option.name ? (
              <>
                {option.name} <small>{option.id}</small>
              </>
            ) : (
              option.id
            )}
          </li>
        ))}
      </ul>
      <span role="status" className="jc-rel-status">
        {said}
      </span>
      {invalid && (
        <span id={errorId} className="jc-rel-error">
          {invalid}
        </span>
      )}
    </div>
  );
}
