// A locale's public groups (vite.config.ts publicLocales, T-5122). Outside src/: a declaration is no module a test can import.
declare module "*.json?public" {
  const locale: import("i18next").ResourceKey;
  export default locale;
}
