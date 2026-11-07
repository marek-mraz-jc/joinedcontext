import { useTranslation } from "react-i18next";
import { SUPPORTED_LOCALES } from "../i18n";
import type { Locale } from "../i18n";
import { offeredLocales, useBranding } from "../branding";
import { Button, Icon, Menu, MenuContent, MenuItem, MenuTrigger } from "./ui";

export function LanguageSwitcher(): React.JSX.Element {
  const { t, i18n } = useTranslation();
  const branding = useBranding();
  const resolved = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const currentLang = (SUPPORTED_LOCALES.includes(resolved as Locale) ? resolved : "sk") as Locale;
  // The installation says which languages it speaks; the bundle says which it can. A locale
  // the bundle has no strings for would switch the page to keys, so the list is the overlap,
  // and an installation that names none keeps every language the bundle carries.
  const offered = offeredLocales(branding).filter((locale): locale is Locale =>
    SUPPORTED_LOCALES.includes(locale as Locale),
  );
  const locales = offered.length > 0 ? offered : [...SUPPORTED_LOCALES];

  return (
    <Menu>
      <MenuTrigger asChild>
        <Button variant="ghost" size="sm" aria-label={t("lang.label")}>
          <Icon name="globe" className="size-4 text-fg-muted" />
          {/* The globe says what the button is at phone width; the language's name is what a
              wider header has room for, and `aria-label` says it to a screen reader either
              way (UI-27). On a touch phone the globe stands alone, without the chevron, so the header's
              controls and the 44 px brand link fit a 360 px phone (T-5596). */}
          <span className="hidden sm:inline">{t(`lang.${currentLang}`)}</span>
          <Icon name="chevronDown" className="size-3.5 text-fg-subtle pointer-coarse:max-sm:hidden" />
        </Button>
      </MenuTrigger>
      <MenuContent align="end" className="min-w-36">
        {locales.map((locale) => {
          const isCurrent = locale === currentLang;
          return (
            <MenuItem
              key={locale}
              aria-current={isCurrent ? "true" : undefined}
              onSelect={() => {
                void i18n.changeLanguage(locale);
              }}
            >
              {isCurrent ? <Icon name="check" className="size-4" /> : <span className="size-4" />}
              {t(`lang.${locale}`)}
            </MenuItem>
          );
        })}
      </MenuContent>
    </Menu>
  );
}
