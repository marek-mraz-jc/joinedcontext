import { useTranslation } from "react-i18next";
export { problemOf } from "../../api/client";

// Shared by the catalogue's pages without loading the catalogue list itself (T-5122).

/** The name of an EU data theme in the reader's language, the catalogue's English one else. */
export function useThemeLabel(): (code: string, fallback?: string) => string {
  const { t } = useTranslation();
  return (code, fallback) => t(`catalogue.theme.${code}`, { defaultValue: fallback ?? code });
}
