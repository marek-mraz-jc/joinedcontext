/**
 * What a dialog imports to draw a form (T-3280): the loader that fetches the form engine with the
 * first form. Vitest maps this module to `SchemaForm` itself (vite.config.ts), so a test reads a
 * form the moment it opens.
 */
export { SchemaForm, preloadSchemaForm } from "./SchemaFormLoader";
