import { test } from "node:test";
import assert from "node:assert/strict";
import {
  shownToolbarStatus,
  toolbarItems,
  toolbarReaction,
  toolbarStatusFrom,
  toolbarTip,
  toolbarUpdateStatus,
} from "../../src/shared/plugin-toolbar.ts";
import { PLUGIN_TOOLBAR_WORDS } from "../../src/renderer/words.ts";
import { UiEvent } from "../../src/shared/ui-events.ts";
import type { PluginInfo, PluginToolbarItem } from "../../src/shared/plugins.ts";

const item = (over: Partial<PluginToolbarItem> = {}): PluginToolbarItem => ({
  id: "demo",
  label: "Example",
  ariaLabel: "Example plugin demo",
  target: { kind: "panel", id: "demo" },
  ...over,
});
const plugin = (id: string, toolbar: PluginToolbarItem[] | undefined, over: Partial<PluginInfo> = {}): PluginInfo => ({
  manifest: {
    apiVersion: 2,
    id,
    version: "1.0.0",
    name: id,
    publisher: "t",
    description: "t",
    backend: "backend.mjs",
    capabilities: [],
    tools: [],
    skills: [],
    panels: [{ id: "demo", title: "Demo", file: "panel.html", placement: "settings" }],
    settings: [],
    actions: [],
    ...(toolbar ? { toolbar } : {}),
  },
  source: "local",
  enabled: true,
  removed: false,
  health: "stopped",
  state: "enabled",
  ...over,
});

test("toolbarItems keeps enabled installed plugins only and honours requiresProject", () => {
  const plugins = [
    plugin("a", [
      item(),
      item({ id: "two", ariaLabel: "Two", requiresProject: false }),
      item({ id: "three", ariaLabel: "Three", requiresProject: true }),
    ]),
    plugin("off", [item()], { enabled: false, state: "disabled" }),
    plugin("gone", [item()], { removed: true, enabled: false, state: "disabled" }),
    plugin("dropped", [item()], { unlisted: true, state: "not-enabled" }),
    plugin("none", undefined),
  ];
  assert.deepEqual(
    toolbarItems(plugins, "game").map((e) => e.key),
    ["a:demo", "a:two", "a:three"],
  );
  assert.deepEqual(
    toolbarItems(plugins, null).map((e) => e.key),
    ["a:two"],
  );
  assert.deepEqual(
    toolbarItems(plugins, undefined).map((e) => e.key),
    ["a:two"],
  );
  assert.deepEqual(
    toolbarItems(plugins, "").map((e) => e.key),
    ["a:two"],
  );
  const [first] = toolbarItems(plugins, "game");
  assert.equal(first!.plugin.manifest.id, "a");
  assert.deepEqual(first!.item, item());
  assert.deepEqual(toolbarItems([], "game"), []);
});
test("toolbarStatusFrom sanitizes badge, title, disabled and tone and rejects non-objects", () => {
  for (const value of [null, undefined, "Draft", 3, true, ["Draft"]]) assert.equal(toolbarStatusFrom(value), null);
  assert.deepEqual(toolbarStatusFrom({}), {});
  assert.deepEqual(toolbarStatusFrom({ badge: "Draft", title: "Draft is online", disabled: false, tone: "ok" }), {
    badge: "Draft",
    title: "Draft is online",
    disabled: false,
    tone: "ok",
  });
  assert.deepEqual(toolbarStatusFrom({ badge: "x".repeat(40) }), { badge: "x".repeat(16) });
  assert.deepEqual(toolbarStatusFrom({ title: "y".repeat(200) }), { title: "y".repeat(120) });
  assert.deepEqual(toolbarStatusFrom({ badge: "  Live \n now  ", title: "\tready\n" }), {
    badge: "Live now",
    title: "ready",
  });
  assert.deepEqual(toolbarStatusFrom({ badge: 7 }), { badge: "7" });
  assert.deepEqual(toolbarStatusFrom({ badge: "", title: "", disabled: 1 }), { disabled: true });
  assert.deepEqual(toolbarStatusFrom({ disabled: "", tone: "loud" }), { disabled: false });
  assert.deepEqual(toolbarStatusFrom({ badge: {}, title: ["x"], tone: "warn", extra: "dropped" }), { tone: "warn" });
  assert.deepEqual(toolbarStatusFrom({ kind: "toolbar", item: "publish", badge: "Live", tone: "ok" }), {
    badge: "Live",
    tone: "ok",
  });
});

test("a change to the open game's own record re-asks the buttons; file writes and other games do not", () => {
  const changed = (payload: { project?: string; file?: string }, open: string | null) =>
    toolbarReaction({ type: UiEvent.GameChanged, payload } as UiEvent, [], open);
  assert.equal(changed({ project: "valley" }, "valley"), "refresh", "its engine link or title changed");
  assert.equal(changed({}, "valley"), "refresh", "every game changed, as when the games folder moves");
  assert.equal(changed({ project: "valley", file: "src/main.ts" }, "valley"), null, "a build writing files");
  assert.equal(changed({ project: "valley", file: "references" }, "valley"), null);
  assert.equal(changed({ project: "lantern" }, "valley"), null, "another game");
  assert.equal(changed({ project: "valley" }, null), null, "no game open");
});

test("a plugin change re-asks every button; a plugin's toolbar event updates its own button", () => {
  const [entry] = toolbarItems([plugin("a", [item()])], "game");
  const event = (payload: unknown) => ({ type: UiEvent.PluginEvent, payload }) as UiEvent;
  assert.equal(toolbarReaction({ type: UiEvent.PluginsChanged, payload: {} } as UiEvent, [], null), "refresh");
  assert.deepEqual(
    toolbarReaction(
      event({ id: "a", event: { kind: "toolbar", item: "demo", badge: "Live", tone: "ok" } }),
      [entry],
      "game",
    ),
    { key: "a:demo", update: { badge: "Live", tone: "ok" } },
  );
  assert.equal(toolbarReaction(event({ id: "a", event: { kind: "toolbar" } }), [entry], "game"), "refresh");
  assert.equal(
    toolbarReaction(event({ id: "b", event: { kind: "toolbar" } }), [entry], "game"),
    null,
    "not its plugin",
  );
  assert.equal(toolbarReaction(event({ id: "a", event: { kind: "progress" } }), [entry], "game"), null);
  assert.equal(toolbarReaction({ type: UiEvent.GameArchived, payload: {} } as UiEvent, [entry], "game"), null);
});

test("a plugin behind the version Studio bundles says Update on its buttons, until that update waits or is done", () => {
  const behind = plugin("unreal", [item()], { availableVersion: "0.3.0" });
  assert.deepEqual(toolbarUpdateStatus(behind, PLUGIN_TOOLBAR_WORDS.update), {
    badge: "Update",
    title: PLUGIN_TOOLBAR_WORDS.update.title("unreal", "0.3.0"),
  });
  const waiting = plugin("unreal", [item()], { availableVersion: "0.3.0", pendingVersion: "0.3.0" });
  assert.equal(toolbarUpdateStatus(waiting, PLUGIN_TOOLBAR_WORDS.update), null, "the update waits for its sessions");
  assert.equal(toolbarUpdateStatus(plugin("unreal", [item()]), PLUGIN_TOOLBAR_WORDS.update), null, "up to date");
  const status = toolbarStatusFrom(toolbarUpdateStatus(behind, PLUGIN_TOOLBAR_WORDS.update));
  assert.equal(status?.badge, "Update", "the badge survives the toolbar's own sanitizing");
});
test("toolbarStatusFrom keeps attention as a boolean", () => {
  assert.deepEqual(toolbarStatusFrom({ attention: true }), { attention: true });
  assert.deepEqual(toolbarStatusFrom({ attention: 0, title: "Up to date" }), { attention: false, title: "Up to date" });
  assert.deepEqual(toolbarStatusFrom({ kind: "toolbar", item: "publish", attention: "yes" }), { attention: true });
  assert.deepEqual(toolbarStatusFrom({ badge: "Draft" }), { badge: "Draft" });
});
test("toolbarStatusFrom keeps dot as a boolean", () => {
  assert.deepEqual(toolbarStatusFrom({ badge: "Ready", tone: "ok", dot: true }), {
    badge: "Ready",
    tone: "ok",
    dot: true,
  });
  assert.deepEqual(toolbarStatusFrom({ badge: "Not open", dot: 0 }), { badge: "Not open", dot: false });
  assert.deepEqual(toolbarStatusFrom({ kind: "toolbar", item: "editor", dot: "yes" }), { dot: true });
});
test("a dot's tooltip says its word first, then the plugin's title; a word badge's says the title alone", () => {
  assert.deepEqual(toolbarTip({ badge: "Not open", tone: "info", dot: true, title: "Drift isn't open in Unreal." }), [
    "Not open",
    "Drift isn't open in Unreal.",
  ]);
  assert.deepEqual(toolbarTip({ badge: "Starting", dot: true }), ["Starting"]);
  assert.deepEqual(toolbarTip({ badge: "Draft", title: "Draft is online" }), ["Draft is online"]);
  assert.deepEqual(toolbarTip({ title: "Make this game in Unreal" }), ["Make this game in Unreal"]);
  assert.deepEqual(toolbarTip({ dot: true }), [], "a dot with no word draws nothing to say");
  assert.deepEqual(toolbarTip(undefined), []);
});
test("Update takes the badge's place as a word in the quiet tone, whatever dot or tone the plugin's own had", () => {
  const own = { badge: "Ready", tone: "ok", dot: true, title: "Connected to Drift." } as const;
  const update = toolbarUpdateStatus(
    plugin("unreal", [item()], { availableVersion: "0.3.0" }),
    PLUGIN_TOOLBAR_WORDS.update,
  );
  assert.deepEqual(shownToolbarStatus(own, update), {
    badge: "Update",
    title: PLUGIN_TOOLBAR_WORDS.update.title("unreal", "0.3.0"),
  });
  assert.deepEqual(shownToolbarStatus({ ...own, disabled: true, attention: true }, update), {
    disabled: true,
    attention: true,
    badge: "Update",
    title: PLUGIN_TOOLBAR_WORDS.update.title("unreal", "0.3.0"),
  });
  assert.equal(shownToolbarStatus(own, null), own, "up to date: the plugin's own status");
  assert.equal(shownToolbarStatus(undefined, null), undefined);
});
