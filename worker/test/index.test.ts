import { expect, test } from "vitest";
import { esc, hardwareHtml, humanSize, parseEvent } from "../src/index";

test("validates events the way server/ does", () => {
  const good = `{"install_id":"abc-123","version":"0.1.0","os":"linux","client":"tui","kind":"scan","files":10,"bytes":2048,"junk_bytes":2048}`;
  expect(parseEvent(good).sizes).toEqual([2048, 0, 2048, 0, 0]);
  expect(() => parseEvent(good.replace("abc-123", "<script>"))).toThrow();
  expect(() => parseEvent(good.replace('"scan"', '"drop"'))).toThrow();
  expect(() => parseEvent(good.replace("10", "-1"))).toThrow();
  expect(() => parseEvent(good.replace('"files":10', '"specs":[]'))).toThrow();

  const open = `{"install_id":"abc-123","version":"0.2.0","os":"macos","client":"gui","kind":"open",
    "specs":[{"kind":"İşlemci","brand":"Apple","name":"Apple M5 Pro <b>","detail":"15 çekirdek"}]}`;
  const ev = parseEvent(open);
  expect(() => parseEvent(open.replace("15 çekirdek", "a\\u0007"))).toThrow();
  const html = hardwareHtml("2026-10-29 10:00:00", ev.specs!);
  expect(html).toContain("[APPLE]");
  expect(html).toContain("Apple M5 Pro &lt;b&gt;");
  expect(html).not.toContain("Pro <b>");
});

test("formats", () => {
  expect(humanSize(2048)).toBe("2.0 KB");
  expect(humanSize(5)).toBe("5 B");
  expect(esc("<x>")).toBe("&lt;x&gt;");
});
