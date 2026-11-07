import { lazy } from "react";
import type { JSX } from "react";
import type { SchemaFormProps } from "./SchemaForm";
import type { SchemaForm as SchemaFormComponent } from "./SchemaForm";

/**
 * The form engine (rjsf, ajv and every widget, the map's included) loads with the first form a
 * person opens, not with every page (T-3280): the dialogs that hold a form, and the assistant's
 * question form on every page, imported it at once and put ~800 KB gzipped in front of each visit.
 *
 * Once loaded it renders at once, so only the very first form waits for it; `preloadSchemaForm`
 * loads it ahead, which is what a test that reads a form as it opens asks for.
 */
let loaded: typeof SchemaFormComponent | undefined;

export function preloadSchemaForm(): Promise<typeof SchemaFormComponent> {
  return import("./SchemaForm").then((module) => {
    loaded = module.SchemaForm;
    return module.SchemaForm;
  });
}

const Waiting = lazy(() => preloadSchemaForm().then((component) => ({ default: component })));

export function SchemaForm<T>(props: SchemaFormProps<T>): JSX.Element {
  const Loaded = loaded;
  if (Loaded) return <Loaded<T> {...props} />;
  const Lazy = Waiting as unknown as typeof SchemaFormComponent;
  return <Lazy<T> {...props} />;
}
