//! deskmonk.app's pages in the visitor's language. The HTML in ../docs is English; elements marked
//! data-i18n="key" (inner HTML) or data-i18n-content="key" (a meta tag's content) are replaced here from i18n.ts,
//! data-price="monthly|yearly" gets the price, and the language menu is filled in. Runs before the static assets
//! only for the paths in wrangler.toml's run_worker_first.

import { LANG_NAMES, LANGS, RTL, USD, money, pickLang, word, type Lang } from "./i18n";
import { esc } from "./admin";

const PAGES = new Set(["/", "/index.html", "/pricing", "/pricing.html"]);

export async function handleSite(req: Request, assets: Fetcher, path: string): Promise<Response | null> {
  if (!PAGES.has(path) || (req.method !== "GET" && req.method !== "HEAD")) return null;
  // Without the browser's If-None-Match: the same URL now has a body per language, so a 304 could show the wrong one.
  const res = await assets.fetch(new Request(req.url, { method: req.method }));
  if (!res.ok || !(res.headers.get("Content-Type") ?? "").includes("text/html")) return res;
  const lang = pickLang(req);
  const out = translate(res, lang, new URL(req.url));
  const headers = new Headers(out.headers);
  headers.delete("ETag");
  headers.set("Cache-Control", "no-cache");
  headers.set("Vary", "Cookie, Accept-Language");
  headers.set("Content-Language", lang);
  if (new URL(req.url).searchParams.has("lang")) headers.append("Set-Cookie", `lang=${lang}; Path=/; Max-Age=31536000; SameSite=Lax; Secure`);
  return new Response(out.body, { status: out.status, headers });
}

export function translate(res: Response, lang: Lang, url: URL): Response {
  const alternates = LANGS.map((code) => `<link rel="alternate" hreflang="${code}" href="${url.origin}${url.pathname}?lang=${code}">`).join("");
  const menu = LANGS.map((code) => `<option value="${code}"${code === lang ? " selected" : ""}>${esc(LANG_NAMES[code])}</option>`).join("");
  return new HTMLRewriter()
    .on("html", { element: (el) => { el.setAttribute("lang", lang); if (RTL.has(lang)) el.setAttribute("dir", "rtl"); } })
    .on("head", { element: (el) => void el.append(`${alternates}<link rel="alternate" hreflang="x-default" href="${url.origin}${url.pathname}">`, { html: true }) })
    // A key missing from en.json keeps the HTML's own English.
    .on("[data-i18n]", { element: (el) => { const w = word(lang, el.getAttribute("data-i18n") ?? ""); if (w) el.setInnerContent(w, { html: true }); } })
    .on("[data-i18n-content]", { element: (el) => { const w = word(lang, el.getAttribute("data-i18n-content") ?? ""); if (w) el.setAttribute("content", w); } })
    .on("[data-price]", { element: (el) => void el.setInnerContent(money(lang, ({ free: 0, monthly: USD.monthly, yearly: USD.yearly } as Record<string, number>)[el.getAttribute("data-price") ?? ""] ?? 0)) })
    .on("select.langs", { element: (el) => void el.setInnerContent(menu, { html: true }) })
    .on('a[href^="/buy"], a[href^="/account"]', {
      element: (el) => {
        const href = el.getAttribute("href") ?? "";
        el.setAttribute("href", `${href}${href.includes("?") ? "&" : "?"}lang=${lang}`);
      },
    })
    .transform(res);
}
