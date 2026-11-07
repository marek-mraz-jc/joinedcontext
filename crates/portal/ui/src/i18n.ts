import i18n from "i18next";
import ICU from "i18next-icu";
import LanguageDetector from "i18next-browser-languagedetector";
import { initReactI18next } from "react-i18next";
import type { BackendModule, ResourceKey } from "i18next";
import en from "./locales/en.json?public";

export const SUPPORTED_LOCALES = ["en", "sk", "de", "cs"] as const;
export type Locale = (typeof SUPPORTED_LOCALES)[number];

type LocaleFile = () => Promise<{ default: ResourceKey }>;

// English is the fallback and ships in the entry; another language loads when it is chosen, so a
// visit downloads one locale besides English rather than all four (T-3316). Until the full Portal
// loads, each language is only its public groups (vite.config.ts publicLocales, T-5122): a
// visitor of the catalogue does not download the words of a hundred signed-in pages.
const PUBLIC: Record<string, LocaleFile> = {
  sk: () => import("./locales/sk.json?public"),
  de: () => import("./locales/de.json?public"),
  cs: () => import("./locales/cs.json?public"),
};
const FULL: Record<string, LocaleFile> = {
  en: () => import("./locales/en.json"),
  sk: () => import("./locales/sk.json"),
  de: () => import("./locales/de.json"),
  cs: () => import("./locales/cs.json"),
};

let full: Promise<void> | null = null;

export const localeLoader: BackendModule = {
  type: "backend",
  init: () => undefined,
  read: (language, _namespace, callback) => {
    const load = (full ? FULL : PUBLIC)[language];
    if (!load) return callback(null, language === "en" ? en : {});
    load().then(
      (locale) => callback(null, locale.default),
      (error: unknown) =>
        callback(error instanceof Error ? error.message : String(error), false),
    );
  },
};

/**
 * Every word of English and of the chosen language, before the full Portal draws a page; once
 * called, a language chosen later loads whole. Called again, it is the same load.
 */
export function loadFullLocales(): Promise<void> {
  full ??= Promise.all(
    [...new Set(["en", i18n.resolvedLanguage ?? i18n.language])].map(async (language) => {
      const load = FULL[language];
      if (load) i18n.addResourceBundle(language, "translation", (await load()).default, true, true);
    }),
  ).then(() => undefined);
  return full;
}

// WCAG 3.1.1: the document language has to follow the chosen locale, not stay at the
// `lang="en"` baked into index.html. Registered before init: init detects `?lang=` and fires
// its first languageChanged inside the call (T-2814).
i18n.on("languageChanged", (lng) => {
  if (typeof document !== "undefined") {
    document.documentElement.lang = lng;
  }
});

/** Settles once the chosen language is loaded (or failed to: English stands in). */
export const i18nReady = i18n
  .use(localeLoader)
  .use(ICU)
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources: { en: { translation: en } },
    partialBundledLanguages: true,
    supportedLngs: SUPPORTED_LOCALES,
    fallbackLng: "en",
    detection: {
      order: ["querystring", "localStorage"],
      lookupQuerystring: "lang",
      lookupLocalStorage: "jc-lang",
      caches: ["localStorage"],
    },
    interpolation: {
      escapeValue: false,
    },
    returnNull: false,
    // A public page that asks for a word outside the public groups gets the whole locale and is
    // drawn again with it ("added" re-renders), rather than showing the key.
    saveMissing: true,
    missingKeyHandler: () => void loadFullLocales(),
    react: { bindI18nStore: "added" },
  });

export default i18n;
