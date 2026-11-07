import { useEffect, useRef, useState } from "react";
import type { ChangeEvent, FocusEvent, KeyboardEvent, JSX } from "react";
import { ariaDescribedByIds } from "@rjsf/utils";
import type { WidgetProps } from "@rjsf/utils";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { searchEntities } from "../../../api/gateway";
import type { GatewayEntity } from "../../../api/gateway";
import { InlineError, Input } from "../../ui";
import { useShownErrors } from "../touched";

/** An NGSI-LD entity id, as a person pastes one. */
const URN = /^urn:ngsi-ld:\S+$/;

export function EntityPicker(props: WidgetProps): JSX.Element {
  const {
    id,
    value,
    required,
    disabled,
    readonly,
    onChange,
    onBlur,
    onFocus,
    options,
    rawErrors,
    placeholder,
  } = props;

  const { t } = useTranslation();

  const space = typeof options?.space === "string" ? options.space : undefined;
  const entityType =
    typeof options?.entityType === "string" ? options.entityType : undefined;

  const isConfigured = Boolean(space && entityType);
  // Set by the entity selector field (UI-03): the space and the type come from the form and may
  // simply not be chosen yet, which is a step still to take, not a misconfigured field.
  const dependent = options?.dependent === true;
  const hasErrors = Boolean(useShownErrors(id, rawErrors));

  const containerRef = useRef<HTMLDivElement>(null);
  const abortControllerRef = useRef<AbortController | null>(null);

  // `typed` is the search term while the user is editing and null the rest of the time, so the
  // visible text can be derived from the selected value instead of synchronised by an effect.
  const [typed, setTyped] = useState<string | null>(null);
  const [debouncedTerm, setDebouncedTerm] = useState("");
  const [isOpen, setIsOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(-1);

  // Debounce typing by 250ms with AbortController cancellation on keystroke / unmount
  useEffect(() => {
    abortControllerRef.current?.abort();
    const controller = new AbortController();
    abortControllerRef.current = controller;

    const timer = setTimeout(() => {
      setDebouncedTerm(typed ?? "");
    }, 250);

    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [typed]);

  const {
    data: entities = [],
    isLoading,
    isError,
  } = useQuery<GatewayEntity[]>({
    queryKey: ["gateway", space, entityType, debouncedTerm],
    queryFn: ({ signal }) => {
      const activeSignal =
        typeof AbortSignal.any === "function" && abortControllerRef.current
          ? AbortSignal.any([signal, abortControllerRef.current.signal])
          : signal;
      return searchEntities({
        space: space!,
        type: entityType!,
        q: debouncedTerm,
        signal: activeSignal,
      });
    },
    enabled: isConfigured,
    staleTime: 30_000,
  });

  // The label of the URN currently in form state; falls back to the URN until the gateway
  // has told us its human-readable name.
  const selectedUrn = typeof value === "string" ? value : "";
  const selectedLabel = selectedUrn
    ? (entities.find((entity) => entity.id === selectedUrn)?.name ?? selectedUrn)
    : "";
  const displayValue = typed ?? selectedLabel;

  // Close combobox when clicking outside
  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (
        containerRef.current &&
        !containerRef.current.contains(event.target as Node)
      ) {
        setIsOpen(false);
        setActiveIndex(-1);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, []);

  if (!isConfigured && dependent) {
    // Nothing to list yet: the id can still be typed or pasted, as before the picker existed.
    return (
      <div className="relative">
        <Input
          id={id}
          autoComplete="off"
          placeholder={placeholder}
          disabled={disabled}
          readOnly={readonly}
          required={required}
          value={typeof value === "string" ? value : ""}
          aria-describedby={`${ariaDescribedByIds(id)} ${id}__pending`}
          aria-invalid={hasErrors ? "true" : undefined}
          onChange={(e) => onChange(e.target.value.trim() === "" ? undefined : e.target.value)}
          onBlur={() => onBlur?.(id, value)}
          onFocus={() => onFocus?.(id, value)}
        />
        <p id={`${id}__pending`} className="mt-1 text-caption text-fg-muted">
          {t("form.entityPickerPending")}
        </p>
      </div>
    );
  }

  if (!isConfigured) {
    return (
      <div className="relative">
        <Input
          id={id}
          disabled
          aria-disabled="true"
          readOnly={readonly}
          value={typeof value === "string" ? value : ""}
          aria-describedby={ariaDescribedByIds(id)}
          aria-invalid={hasErrors ? "true" : undefined}
        />
        <InlineError className="mt-1">
          {t("form.invalid")}
        </InlineError>
      </div>
    );
  }

  const selectEntity = (entity: GatewayEntity) => {
    setTyped(null);
    onChange(entity.id);
    setIsOpen(false);
    setActiveIndex(-1);
  };

  const handleInputChange = (e: ChangeEvent<HTMLInputElement>) => {
    const next = e.target.value;
    setTyped(next);
    setIsOpen(true);
    setActiveIndex(-1);
    if (!next.trim()) {
      onChange(undefined);
    }
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (disabled || readonly) {
      return;
    }

    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (!isOpen) {
        setIsOpen(true);
      } else if (entities.length > 0) {
        setActiveIndex((prev) => (prev + 1 < entities.length ? prev + 1 : 0));
      }
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (!isOpen) {
        setIsOpen(true);
      } else if (entities.length > 0) {
        setActiveIndex((prev) => (prev > 0 ? prev - 1 : entities.length - 1));
      }
    } else if (e.key === "Enter") {
      if (typedUrn && activeIndex < 0) {
        e.preventDefault();
        setTyped(null);
        onChange(typedUrn);
        setIsOpen(false);
      } else if (isOpen && entities.length > 0) {
        e.preventDefault();
        const chosen = activeIndex >= 0 ? entities[activeIndex] : entities[0];
        if (chosen) {
          selectEntity(chosen);
        }
      }
    } else if (e.key === "Escape") {
      e.preventDefault();
      setIsOpen(false);
      setActiveIndex(-1);
    }
  };

  const handleFocus = () => {
    setIsOpen(true);
    onFocus?.(id, value);
  };

  // An id typed or pasted whole is taken as it is: the list offers what the search found, and a
  // person who already holds the URN should not have to find it by name.
  const typedUrn = typed !== null && URN.test(typed.trim()) ? typed.trim() : undefined;

  const handleBlur = (e: FocusEvent<HTMLInputElement>) => {
    if (!containerRef.current?.contains(e.relatedTarget as Node)) {
      setIsOpen(false);
      setActiveIndex(-1);
      if (typedUrn) {
        setTyped(null);
        onChange(typedUrn);
      }
    }
    onBlur?.(id, value);
  };

  const activeOptionId =
    isOpen && activeIndex >= 0 && activeIndex < entities.length
      ? `${id}__option-${activeIndex}`
      : undefined;

  return (
    <div ref={containerRef} className="relative">
      <Input
        id={id}
        role="combobox"
        autoComplete="off"
        aria-autocomplete="list"
        aria-expanded={isOpen ? "true" : "false"}
        // Only while the list is there: an id nothing in the page carries is an invalid reference.
        aria-controls={isOpen && !isLoading && !isError ? `${id}__listbox` : undefined}
        aria-activedescendant={activeOptionId}
        // The Field renders the description, the hint and the errors above and below; nothing
        // else points this control at them (T-2314, UI-04).
        aria-describedby={ariaDescribedByIds(id)}
        aria-invalid={hasErrors ? "true" : undefined}
        required={required}
        disabled={disabled}
        readOnly={readonly}
        value={displayValue}
        placeholder={placeholder}
        onChange={handleInputChange}
        onKeyDown={handleKeyDown}
        onFocus={handleFocus}
        onBlur={handleBlur}
      />

      {isOpen && (
        <div className="absolute z-10 mt-1 max-h-60 w-full overflow-auto rounded border border-border bg-surface shadow-lg">
          {isLoading && (
            <div role="status" className="p-2 text-sm text-surface-fg/70">
              {t("app.loading")}
            </div>
          )}
          {isError && (
            <div role="alert" className="p-2 text-sm text-danger">
              {t("app.error.generic")}
            </div>
          )}
          {!isLoading && !isError && (
            <ul id={`${id}__listbox`} role="listbox" className="w-full">
              {entities.length === 0 ? (
                <li
                  role="option"
                  aria-selected="false"
                  aria-disabled="true"
                  className="p-2 text-sm text-surface-fg/70"
                >
                  {t("form.noResults")}
                </li>
              ) : (
                entities.map((entity, index) => {
                  const isSelected = entity.id === value;
                  const isActive = index === activeIndex;
                  return (
                    <li
                      key={entity.id}
                      id={`${id}__option-${index}`}
                      role="option"
                      aria-selected={isSelected}
                      className={clsx(
                        "cursor-pointer px-3 py-2 text-sm text-surface-fg",
                        isActive && "bg-surface-subtle",
                        isSelected && "font-semibold"
                      )}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => selectEntity(entity)}
                    >
                      {entity.name ?? entity.id}
                    </li>
                  );
                })
              )}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
