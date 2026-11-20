import type { Language, UiLanguage } from './api';

const browserLocale = navigator.language;
const browserLanguage = browserLocale.toLowerCase().split('-')[0];
const fallbackLanguage: UiLanguage = browserLanguage === 'zh' ? 'zh-CN' : 'en';
const active = $state({ language: fallbackLanguage });

export function setLanguage(preference: Language, systemLanguage = fallbackLanguage)
{
  const language = preference === 'system' ? systemLanguage : preference;
  active.language = language;
  document.documentElement.lang = language;
}

export const isChinese = () => active.language === 'zh-CN';
export const local = (english: string, chinese: string) => isChinese() ? chinese : english;

export function locale()
{
  const language = active.language;
  return browserLanguage === language.slice(0, 2) ? browserLocale : language;
}

export function localized<T>(create: (locale: string) => T): () => T
{
  // Reuse Intl formatters until the selected language changes.
  const value = $derived(create(locale()));
  return () => value;
}
