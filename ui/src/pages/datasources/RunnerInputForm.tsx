import { useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { SchemaForm } from "../../components/forms/SchemaForm";
import type { ErrorSchema } from "@rjsf/utils";
import type { CatalogInput } from "../../schemas/kinds";
import { runnerInputSchema } from "../../schemas/kinds";
import { SecretRefContext } from "../../components/forms/widgets/SecretRef";
import type { SecretRefValue } from "../../components/forms/widgets/SecretRef";

export interface BentoCatalog {
  version: string;
  inputs: CatalogInput[];
}

let cachedCatalog: BentoCatalog | undefined;

/** Lazily imports the Bento inputs catalog (~280 KB) so initial page bundle is small. */
export function useBentoInputs(): BentoCatalog | undefined {
  const [catalog, setCatalog] = useState<BentoCatalog | undefined>(cachedCatalog);

  useEffect(() => {
    if (cachedCatalog) return;
    let active = true;
    void import("../../schemas/bento-inputs.json").then((mod) => {
      cachedCatalog = mod.default as BentoCatalog;
      if (active) {
        setCatalog(cachedCatalog);
      }
    });
    return () => {
      active = false;
    };
  }, []);

  return catalog;
}

export interface RunnerInputFormProps {
  input: CatalogInput;
  formData?: Record<string, unknown>;
  disabled?: boolean;
  submitLabel?: string;
  /** The submit shows it is working and takes no second press while the run is starting. */
  submitting?: boolean;
  /** Why this person may not start a run, on the button itself rather than in a 403 (UI-44). */
  submitDisabledReason?: string;
  /** What the server refused, by field, so the message lands beside the field it belongs to. */
  extraErrors?: ErrorSchema;
  knownSecretNames?: string[];
  secrets?: SecretRefValue[];
  onSecretRef?: (envVar: string, ref: SecretRefValue) => void;
  onSubmit: (data: Record<string, unknown>) => void;
  onChange?: (data: Record<string, unknown> | undefined) => void;
}

/** Form generated from a runner input's field tree (T-0476, PL-50). */
export function RunnerInputForm({
  input,
  formData,
  disabled,
  submitLabel,
  submitting,
  submitDisabledReason,
  extraErrors,
  knownSecretNames = [],
  secrets = [],
  onSecretRef,
  onSubmit,
  onChange,
}: RunnerInputFormProps): JSX.Element {
  const { schema, uiSchema } = useMemo(() => runnerInputSchema(input), [input]);

  const contextValue = useMemo(
    () => ({
      knownSecretNames,
      secrets,
      onSecretRef,
    }),
    [knownSecretNames, secrets, onSecretRef]
  );

  return (
    <SecretRefContext.Provider value={contextValue}>
      <SchemaForm<Record<string, unknown>>
        schema={schema}
        uiSchema={uiSchema}
        formData={formData}
        disabled={disabled}
        submitLabel={submitLabel}
        submitting={submitting}
        submitDisabledReason={submitDisabledReason}
        extraErrors={extraErrors}
        onSubmit={onSubmit}
        onChange={onChange}
      />
    </SecretRefContext.Provider>
  );
}
