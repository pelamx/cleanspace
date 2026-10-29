//! Languages of deskmonk.app and the buyer pages. Texts are in ../i18n/<code>.json, the same keys as en.json;
//! a missing key falls back to English. Prices are not in the texts: {monthly} and {yearly}
//! are filled in from USD, formatted the way the language writes money.

import ar from "../i18n/ar.json";
import az from "../i18n/az.json";
import bg from "../i18n/bg.json";
import bn from "../i18n/bn.json";
import cs from "../i18n/cs.json";
import da from "../i18n/da.json";
import de from "../i18n/de.json";
import el from "../i18n/el.json";
import en from "../i18n/en.json";
import es from "../i18n/es.json";
import fa from "../i18n/fa.json";
import fi from "../i18n/fi.json";
import fr from "../i18n/fr.json";
import he from "../i18n/he.json";
import hi from "../i18n/hi.json";
import hu from "../i18n/hu.json";
import id from "../i18n/id.json";
import it from "../i18n/it.json";
import ja from "../i18n/ja.json";
import ko from "../i18n/ko.json";
import ms from "../i18n/ms.json";
import nb from "../i18n/nb.json";
import nl from "../i18n/nl.json";
import pl from "../i18n/pl.json";
import pt from "../i18n/pt.json";
import ro from "../i18n/ro.json";
import ru from "../i18n/ru.json";
import sv from "../i18n/sv.json";
import th from "../i18n/th.json";
import tr from "../i18n/tr.json";
import uk from "../i18n/uk.json";
import vi from "../i18n/vi.json";
import zh from "../i18n/zh.json";
import zhTW from "../i18n/zh-TW.json";

/** USD list prices of deskMonk Pro (also src/plan.rs). The amounts charged are the Paddle prices; keep them equal. */
export const USD = { monthly: 2.5, yearly: 25 };

type Dict = Record<string, string>;
const DICTS: Record<string, Dict> = {
  en, tr, es, de, fr, it, pt, nl, pl, cs, hu, ro, bg, ru, uk, el, az, fi,
  ar, he, fa, hi, bn, ja, ko, zh, "zh-TW": zhTW, th, id, ms, vi, sv, da, nb,
};

/** Code → name in that language, in menu order. */
export const LANG_NAMES: Record<string, string> = {
  en: "English", tr: "Türkçe", es: "Español", de: "Deutsch", fr: "Français", it: "Italiano", pt: "Português", nl: "Nederlands",
  pl: "Polski", cs: "Čeština", hu: "Magyar", ro: "Română", bg: "Български", ru: "Русский", uk: "Українська", el: "Ελληνικά",
  az: "Azərbaycanca", fi: "Suomi", sv: "Svenska", da: "Dansk", nb: "Norsk", ar: "العربية", he: "עברית", fa: "فارسی",
  hi: "हिन्दी", bn: "বাংলা", id: "Bahasa Indonesia", ms: "Bahasa Melayu", vi: "Tiếng Việt", th: "ไทย", ja: "日本語", ko: "한국어",
  zh: "简体中文", "zh-TW": "繁體中文",
};
export type Lang = string;
export const LANGS: Lang[] = Object.keys(LANG_NAMES);
export const RTL = new Set(["ar", "he", "fa"]);

/** The language most people in a country read (ISO 3166-1 alpha-2). Countries not listed: the browser's language. */
const BY_COUNTRY: Record<string, Lang> = {};
const COUNTRIES: Record<Lang, string> = {
  tr: "TR CY",
  az: "AZ",
  es: "ES MX AR CO CL PE VE EC GT CU BO DO HN PY SV NI CR PA UY PR GQ",
  de: "DE AT CH LI",
  fr: "FR BE LU MC SN CI CM CD CG MG ML BF NE TD GN BJ TG HT GA DJ KM RW BI",
  it: "IT SM VA",
  pt: "PT BR AO MZ CV GW ST TL",
  nl: "NL SR AW CW",
  pl: "PL",
  cs: "CZ",
  hu: "HU",
  ro: "RO MD",
  bg: "BG",
  ru: "RU BY KZ KG TJ",
  uk: "UA",
  el: "GR",
  fi: "FI",
  sv: "SE AX",
  da: "DK GL FO",
  nb: "NO SJ",
  ar: "SA AE EG MA DZ TN LY JO LB SY IQ KW QA BH OM YE SD PS MR",
  he: "IL",
  fa: "IR AF",
  hi: "IN",
  bn: "BD",
  id: "ID",
  ms: "MY BN",
  vi: "VN",
  th: "TH",
  ja: "JP",
  ko: "KR",
  zh: "CN SG",
  "zh-TW": "TW HK MO",
};
for (const [lang, list] of Object.entries(COUNTRIES)) for (const c of list.split(" ")) BY_COUNTRY[c] = lang;
export const langOfCountry = (country: string | null): Lang | null => (country ? BY_COUNTRY[country] ?? null : null);

/** "zh-Hant-TW", "zh-HK" → zh-TW; "pt-BR" → pt; "no"/"nn" → nb; unknown → null. */
export function langOfTag(tag: string): Lang | null {
  const t = tag.trim().toLowerCase();
  if (/^zh-(tw|hk|mo|hant)/.test(t)) return "zh-TW";
  const base = t.split(/[-_]/)[0];
  if (base === "no" || base === "nn") return "nb";
  return LANGS.includes(base) ? base : null;
}

/** The visitor's Cloudflare country (from the IP; the address itself is not kept), or null for unknown/Tor. */
export function countryOf(req: Request): string | null {
  const c = (req as { cf?: { country?: unknown } }).cf?.country;
  return typeof c === "string" && /^[A-Z]{2}$/.test(c) && c !== "XX" && c !== "T1" ? c : null;
}

/** ?lang=, then the visitor's own choice (the "lang" cookie), then their country, then the browser language, then English. */
export function pickLang(req: Request): Lang {
  const q = new URL(req.url).searchParams.get("lang");
  if (q && LANGS.includes(q)) return q;
  const c = /(?:^|; )lang=([A-Za-z-]{2,5})(?:;|$)/.exec(req.headers.get("Cookie") ?? "");
  if (c && LANGS.includes(c[1])) return c[1];
  const byCountry = langOfCountry(countryOf(req));
  if (byCountry) return byCountry;
  for (const part of (req.headers.get("Accept-Language") ?? "").split(",")) {
    const lang = langOfTag(part.split(";")[0]);
    if (lang) return lang;
  }
  return "en";
}

export function money(lang: Lang, usd: number): string {
  return new Intl.NumberFormat(lang, { style: "currency", currency: "USD", minimumFractionDigits: Number.isInteger(usd) ? 0 : 2 }).format(usd);
}

/** The text for `key` in `lang` (English if missing), with the price placeholders filled in. */
export function word(lang: Lang, key: string): string {
  const text = DICTS[lang]?.[key] ?? (en as Dict)[key] ?? "";
  if (!text.includes("{")) return text;
  return text
    .replaceAll("{monthly}", money(lang, USD.monthly))
    .replaceAll("{yearly}", money(lang, USD.yearly));
}

/** Every key with this prefix, prefix removed, in `lang`. */
export function words(lang: Lang, prefix: string): Dict {
  const out: Dict = {};
  for (const key of Object.keys(en)) if (key.startsWith(prefix)) out[key.slice(prefix.length)] = word(lang, key);
  return out;
}
