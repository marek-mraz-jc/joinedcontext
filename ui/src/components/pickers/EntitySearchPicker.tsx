import { reasonOf } from "../../api/client";
import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { searchEntities } from "../../api/gateway";
import { Combobox } from "./Combobox";
import { useDebounced } from "./organizationModels";

export interface EntitySearchPickerProps {
  id?: string;
  label: string;
  labelled?: boolean;
  /** The context space searched, through the caller's own session. */
  space: string;
  /** The type the relationship points to. */
  type: string;
  value: string;
  onChange: (id: string) => void;
  required?: boolean;
  invalid?: boolean;
  describedBy?: string;
  /** Stamped by a wrapping `Field` (see `Combobox`). */
  "aria-describedby"?: string;
}

/**
 * An entity a relationship points to, found by name in the space (T-4946): the gateway searches
 * with the caller's own session, so their Policy decides which entities are listed, and the write
 * that names one is checked by the same Policy and the model.
 */
export function EntitySearchPicker({
  id,
  label,
  labelled,
  space,
  type,
  value,
  onChange,
  required,
  invalid,
  describedBy,
  "aria-describedby": fieldDescribedBy,
}: EntitySearchPickerProps): JSX.Element {
  const { t } = useTranslation();
  const [text, setText] = useState("");
  const term = useDebounced(text);
  const found = useQuery({
    queryKey: ["entitySearch", space, type, term],
    queryFn: ({ signal }) => searchEntities({ space, type, q: term, limit: 20, signal }),
    staleTime: 30_000,
  });
  return (
    <Combobox
      id={id}
      label={label}
      labelled={labelled}
      value={value ? [value] : []}
      options={(found.data ?? []).map((entity) => ({ value: entity.id, label: entity.name ?? entity.id, detail: entity.id, group: type }))}
      searched
      onSearch={setText}
      loading={found.isLoading}
      failed={found.isError ? { reason: reasonOf(found.error), retry: () => void found.refetch() } : undefined}
      empty={term ? t("picker.entity.noMatch", { type }) : t("picker.entity.none", { type })}
      onChange={(values) => onChange(values[0] ?? "")}
      required={required}
      invalid={invalid}
      describedBy={describedBy}
      aria-describedby={fieldDescribedBy}
    />
  );
}
