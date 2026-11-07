import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    // The embed script a site loads with a <script> tag (API/01 §33): plain browser JavaScript.
    files: ["public/**/*.js"],
    languageOptions: { sourceType: "script", globals: { ...globals.browser } },
  },
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      globals: {
        ...globals.browser,
      },
    },
    plugins: {
      "react-hooks": reactHooks,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // `_name` means "declared on purpose and not read" — a mock whose call tuple is typed from
      // its signature, a test table's unused first column. Thirteen places already write it and
      // lint passed, because the rule's default `args: "after-used"` only forgives an unused
      // argument that a used one follows; two trailing ones turned red on main (T-2325). This
      // says the convention out loud instead of leaving it to argument order. Only the underscore
      // is forgiven: an ordinary unused variable is still an error.
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrorsIgnorePattern: "^_" },
      ],
    },
  },
  {
    // T-1726: the same rules `tests/ui_rules.test.ts` enforces, said in the editor while the line
    // is being written. The test is the gate — it reads every file and holds the allow-list —
    // and these are the ones ESLint can see from the syntax tree alone. `components/ui` is where
    // a control is built, so the controls are refused everywhere else.
    files: ["src/**/*.{ts,tsx}"],
    ignores: ["src/components/ui/**"],
    rules: {
      // A warning, not an error: `tests/ui_rules.test.ts` is the gate and holds the allow-list of
      // what is already here, which ESLint has no way to express. This says it in the editor on
      // the line being written, without turning 108 known lines into a red build.
      "no-restricted-syntax": [
        "warn",
        {
          selector:
            "JSXOpeningElement > JSXIdentifier[name=/^(button|input|select|textarea|table)$/]",
          message:
            "Use the shared control from components/ui: a page does not hand-make a control (UI-16).",
        },
        {
          selector: "JSXAttribute[name.name='style']",
          message: "Use a token class: a size and a colour come from the scale (UI-01).",
        },
        {
          selector: "JSXAttribute[name.name='autoFocus']",
          message: "Do not take focus on arrival: it moves a screen reader mid-sentence (UI-01).",
        },
        {
          selector: "TSAsExpression > TSNeverKeyword",
          message:
            "Type it from the API's schema or the route's search, not `as never` (T-1488; tests/ui_rules.test.ts fails on it).",
        },
        {
          selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']",
          message: "Markup is never built from a string (PF-50).",
        },
        {
          selector:
            "CallExpression[callee.object.name='window'][callee.property.name=/^(confirm|alert|open)$/]",
          message:
            "Ask with the shared dialog, and open a link with an anchor that carries rel (UI-16, PF-50).",
        },
      ],
    },
  },
  {
    files: ["scripts/**/*.mjs"],
    languageOptions: { globals: { ...globals.node } },
  }
);
