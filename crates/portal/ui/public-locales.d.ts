// A locale's public groups (vite.config.ts publicLocales, T-5122). Outside src/: a declaration is no module a test can import.
declare module "*.json?public" {
  const locale: import("i18next").ResourceKey;
  export default locale;
}

// The shared grid's groups alone (vite.config.ts GRID_GROUPS, T-6752).
declare module "*.json?grid" {
  const locale: import("i18next").ResourceKey;
  export default locale;
}
