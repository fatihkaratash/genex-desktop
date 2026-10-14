import type { PluginInfo, PluginToolbarItem, PluginToolbarStatus } from "./plugins.ts";
import { UiEvent } from "./ui-events.ts";

/** One button contributed to the stage strip; `key` is the value of `data-plugin-toolbar`. */
export interface PluginToolbarEntry {
  key: string;
  plugin: PluginInfo;
  item: PluginToolbarItem;
}
const TONES = new Set(["ok", "warn", "err", "info"]);

/** Buttons to render for the current project: enabled, installed plugins only; items that need a project wait for one. */
export function toolbarItems(plugins: readonly PluginInfo[], project: string | null | undefined): PluginToolbarEntry[] {
  const entries: PluginToolbarEntry[] = [];
  for (const plugin of plugins) {
    if (!plugin.enabled || plugin.removed || plugin.unlisted) continue;
    for (const item of plugin.manifest.toolbar ?? []) {
      if (item.requiresProject !== false && !project) continue;
      entries.push({ key: `${plugin.manifest.id}:${item.id}`, plugin, item });
    }
  }
  return entries;
}

const text = (value: unknown, max: number): string | undefined => {
  if (typeof value !== "string" && typeof value !== "number") return undefined;
  const clean = String(value).replace(/\s+/g, " ").trim().slice(0, max);
  return clean || undefined;
};

/** A plugin's own `plugin.event` value addressed to its toolbar: `{kind: "toolbar", item?, …status}`. */
export function isToolbarEvent(value: unknown): value is { kind: "toolbar"; item?: unknown } {
  return typeof value === "object" && value !== null && "kind" in value && value.kind === "toolbar";
}

/** Sanitize a status returned by a plugin action or pushed through a `plugin.event` of kind `toolbar`. */
export function toolbarStatusFrom(value: unknown): PluginToolbarStatus | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const raw = value as Record<string, unknown>,
    status: PluginToolbarStatus = {};
  const badge = text(raw.badge, 16),
    title = text(raw.title, 120);
  if (badge !== undefined) status.badge = badge;
  if (title !== undefined) status.title = title;
  if (raw.disabled !== undefined) status.disabled = Boolean(raw.disabled);
  if (raw.attention !== undefined) status.attention = Boolean(raw.attention);
  if (raw.dot !== undefined) status.dot = Boolean(raw.dot);
  if (typeof raw.tone === "string" && TONES.has(raw.tone)) status.tone = raw.tone as PluginToolbarStatus["tone"];
  return status;
}

/** The words a button says while its plugin is behind the version Studio bundles: a badge and its tooltip. */
export interface ToolbarUpdateWords {
  badge: string;
  title: (plugin: string, version: string) => string;
}

/**
 * What a plugin's buttons say while Studio bundles a newer version of it than the one installed:
 * an Update badge naming that version, which the toolbar shows over the plugin's own status. Null
 * once the update is applied or waits for the plugin's sessions to end.
 */
export function toolbarUpdateStatus(plugin: PluginInfo, words: ToolbarUpdateWords): PluginToolbarStatus | null {
  const version = plugin.availableVersion;
  if (!version || plugin.pendingVersion) return null;
  return { badge: words.badge, title: words.title(plugin.manifest.name, version) };
}

/**
 * What a button shows: its plugin's own status, or while an update is offered the Update badge
 * over it, as a word in the quiet tone whatever look the plugin's own badge had.
 */
export function shownToolbarStatus(
  own: PluginToolbarStatus | undefined,
  update: PluginToolbarStatus | null,
): PluginToolbarStatus | undefined {
  if (!update) return own;
  const { badge: _badge, tone: _tone, dot: _dot, ...rest } = own ?? {};
  return { ...rest, ...update };
}

/** A button's tooltip lines: a dot's word first, since the dot alone doesn't say it, then the plugin's title. */
export function toolbarTip(status: PluginToolbarStatus | undefined): string[] {
  const word = status?.dot ? status.badge : undefined;
  return [word, status?.title].filter((line): line is string => Boolean(line));
}

/**
 * Whether a `game.changed` payload says the open game's own record changed (its engine link, its
 * title), which a button's status may follow. A build's file writes each carry `file` and are not
 * asked about.
 */
function gameRecordChanged(payload: { project?: string; file?: string } | undefined, project: string | null): boolean {
  if (!payload || !project || payload.file) return false;
  return payload.project === undefined || payload.project === project;
}

/**
 * What a plugin's `toolbar` event changes: one button's status in place, or — when it names no
 * button of this toolbar or carries no status — a refresh of them all. Null for anything else.
 */
function toolbarChange(
  payload: { id: string; event: unknown } | undefined,
  entries: readonly PluginToolbarEntry[],
): { key: string; update: PluginToolbarStatus } | "refresh" | null {
  const raw = payload?.event;
  if (!payload || typeof payload.id !== "string") return null;
  if (!isToolbarEvent(raw)) return null;
  const mine = entries.filter((e) => e.plugin.manifest.id === payload.id);
  if (mine.length === 0) return null;
  const target = typeof raw.item === "string" ? mine.find((e) => e.item.id === raw.item) : undefined;
  const update = toolbarStatusFrom(raw);
  return target && update ? { key: target.key, update } : "refresh";
}

/** What a UI event asks of the toolbar: a re-ask of every status, one button's new status, or nothing. */
export function toolbarReaction(
  event: UiEvent,
  entries: readonly PluginToolbarEntry[],
  project: string | null,
): { key: string; update: PluginToolbarStatus } | "refresh" | null {
  if (event.type === UiEvent.PluginsChanged) return "refresh";
  if (event.type === UiEvent.GameChanged) return gameRecordChanged(event.payload, project) ? "refresh" : null;
  return event.type === UiEvent.PluginEvent ? toolbarChange(event.payload, entries) : null;
}
