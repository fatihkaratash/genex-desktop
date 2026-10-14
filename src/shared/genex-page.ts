/**
 * Edits to a web game's entry page that connect it to Genex, as text: Studio's copy of
 * `@genex-ai/embed-sdk` in a template game's import map, the module that connects the page beside
 * it, and the `<head>` tag the games host injects its environment into.
 */
import { GENEX_EMBED_VENDOR } from "./genex.ts";

/** The package a page imports the SDK by. */
const SDK_PACKAGE = "@genex-ai/embed-sdk";

const HEAD_OPEN = /<head(?=[\s>/])[^>]*>/i;
const HTML_OPEN = /<html(?=[\s>/])[^>]*>/i;
const DOCTYPE = /<!doctype[^>]*>/i;
const IMPORT_MAP = /(<script\b[^>]*\btype\s*=\s*["']?importmap["']?[^>]*>)([\s\S]*?)(<\/script\s*>)/gi;
const COMMENT = /<!--[\s\S]*?(?:-->|$)/g;

/** The page with every comment blanked: offsets stay the page's, and a tag inside a comment is not found. */
function visibleTags(html: string): string {
  return html.replace(COMMENT, (comment) => " ".repeat(comment.length));
}

function insertAt(html: string, at: number, text: string): string {
  return html.slice(0, at) + text + html.slice(at);
}

/** Just past the first match of `pattern` in `text`, or -1. */
function endOf(text: string, pattern: RegExp): number {
  const match = pattern.exec(text);
  return match ? match.index + match[0].length : -1;
}

/** Every import map the page really has (none inside a comment), in page order. */
function importMaps(html: string): RegExpExecArray[] {
  return [...visibleTags(html).matchAll(IMPORT_MAP)];
}

/** Where a page loads the connect module from: `vendorBase` is `/vendor` in a game, `./vendor` in a published copy. */
export function connectModuleSrc(vendorBase: string): string {
  return `${vendorBase}/${GENEX_EMBED_VENDOR.Connect}`;
}

/**
 * The page with a `<head>` tag: one already there is kept; otherwise an empty one goes where the
 * parser opens it anyway (after `<html>`, the doctype, or a byte-order mark), so the page's own
 * head content still lands in it and the doctype stays first.
 */
export function withHead(html: string): string {
  const visible = visibleTags(html);
  if (HEAD_OPEN.test(visible)) return html;
  const after = [HTML_OPEN, DOCTYPE].map((pattern) => endOf(visible, pattern)).find((at) => at >= 0);
  const start = html.startsWith("\uFEFF") ? 1 : 0;
  return insertAt(html, after ?? start, "<head></head>");
}

/**
 * The page loading the connect module from `src`: after its last import map (a module before one
 * stops the map from applying), else first in its head, so it runs before any of the game's
 * modules. A page that already loads it is left as it is; one with neither gets a head.
 */
export function withConnectScript(html: string, src: string): string {
  const tag = `<script type="module" src="${src}"></script>`;
  if (visibleTags(html).includes(GENEX_EMBED_VENDOR.Connect)) return html;
  const lastMap = importMaps(html).at(-1);
  if (lastMap) return insertAt(html, lastMap.index + lastMap[0].length, tag);
  const page = withHead(html);
  return insertAt(page, endOf(visibleTags(page), HEAD_OPEN), tag);
}

/** Whether a game's page loads the Genex SDK through Studio's copy: its import map names it, or it loads the connect module. */
export function pageLoadsGenexSdk(html: string): boolean {
  const visible = visibleTags(html);
  if (visible.includes(GENEX_EMBED_VENDOR.Connect)) return true;
  return importMaps(html).some(([, , body = ""]) => {
    try {
      return Object.hasOwn(JSON.parse(body)?.imports ?? {}, SDK_PACKAGE);
    } catch {
      return false;
    }
  });
}

/** The indentation of the first line of `body` that has text on it. */
function indentOf(body: string): string {
  return /^([ \t]*)\S/m.exec(body)?.[1] ?? "";
}

/**
 * A template game's page with Studio's copy of the SDK in its first import map, at `/vendor`, and
 * the connect module loaded right after the map. Null when the page has no import map, or one that
 * does not parse or is scoped: such a page is not the template's, and is not edited.
 */
export function withGenexSdkImport(html: string): string | null {
  const [found] = importMaps(html);
  if (!found) return null;
  // The groups are read from the page itself: the search ran on a copy with its comments blanked.
  const whole = html.slice(found.index, found.index + found[0].length);
  const [, open = "", body = "", close = ""] = new RegExp(IMPORT_MAP.source, "i").exec(whole) ?? [];
  let map: { imports?: Record<string, string>; scopes?: unknown };
  try {
    map = JSON.parse(body);
  } catch {
    return null;
  }
  if (map === null || typeof map !== "object" || map.scopes !== undefined) return null;
  if (Object.hasOwn(map.imports ?? {}, SDK_PACKAGE)) return withConnectScript(html, connectModuleSrc("/vendor"));
  const imports = { ...map.imports, [SDK_PACKAGE]: `/vendor/${GENEX_EMBED_VENDOR.Entry}` };
  const indent = indentOf(body);
  const json = JSON.stringify({ ...map, imports }, null, 2)
    .split("\n")
    .map((line) => indent + line)
    .join("\n");
  const closeIndent = /\n([ \t]*)$/.exec(body)?.[1] ?? "";
  const rewritten = `${open}\n${json}\n${closeIndent}${close}`;
  const page = html.slice(0, found.index) + rewritten + html.slice(found.index + whole.length);
  return withConnectScript(page, connectModuleSrc("/vendor"));
}
