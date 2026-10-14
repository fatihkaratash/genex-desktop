import path from "node:path";
import { createHash } from "node:crypto";
import { readdir, readFile, writeFile } from "node:fs/promises";
import { SECOND_MS } from "../../shared/duration.ts";
import { withHead } from "../../shared/genex-page.ts";

export const DEPLOYMENT_MARKER = "studio-deployment.json";
const ENTRY_FILE = "index.html";
/** How long the whole hosted-content check may take, and one file's read within it. */
const VERIFY_TIMEOUT_MS = 45 * SECOND_MS;
const FILE_READ_TIMEOUT_MS = 15 * SECOND_MS;
/** Hosted files read at once while verifying. */
const VERIFY_CONCURRENCY = 4;

/** Why an export cannot be marked, or a hosted deployment does not match it. */
const MESSAGE = {
  SymbolicLink: (relative: string) => `Export contains a symbolic link: ${relative}`,
  InvalidIdentity: "Invalid deployment identity",
  NoEntryPage: "Export has no index.html",
  ReadFailed: (relative: string, status: number) => `${relative}: HTTP ${status}`,
  StillServingPrevious: "The previous deployment is still being served",
  NotReached: (file: string) => `${file}: hosted content has not reached this upload`,
} as const;

export interface DeploymentManifest {
  id: string;
  digest: string;
  files: Array<{ path: string; sha256: string }>;
}
const digest = (data: Uint8Array | string) => createHash("sha256").update(data).digest("hex");
/** The entry page's marker naming one upload. */
const deploymentTag = (id: string) => `<meta name="studio-deployment" content="${id}">`;

/**
 * Put this upload's marker first in the export's head, replacing any earlier one. A page with no
 * head tag gets one ({@link withHead}): the games host injects only into a head, and a marker ahead
 * of the doctype would put the page in quirks mode.
 */
async function tagEntry(root: string, id: string): Promise<void> {
  const entry = path.join(root, ENTRY_FILE);
  const html = (await readFile(entry, "utf8")).replace(/<meta name="studio-deployment" content="[^"]*">/g, "");
  const tag = deploymentTag(id);
  await writeFile(
    entry,
    withHead(html).replace(/<head(?=[\s>/])[^>]*>/i, (head) => head + tag),
  );
}

/** Every file of the export (hidden ones and the marker excepted) with its digest; a symlink is refused. */
async function exportFiles(root: string): Promise<DeploymentManifest["files"]> {
  const files: DeploymentManifest["files"] = [];
  async function visit(dir: string) {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      if (entry.name.startsWith(".")) continue;
      const full = path.join(dir, entry.name);
      const relative = path.relative(root, full).split(path.sep).join("/");
      if (entry.isSymbolicLink()) throw new Error(MESSAGE.SymbolicLink(relative));
      if (entry.isDirectory()) await visit(full);
      else if (entry.isFile() && relative !== DEPLOYMENT_MARKER)
        files.push({ path: relative, sha256: digest(await readFile(full)) });
    }
  }
  await visit(root);
  return files.sort((a, b) => a.path.localeCompare(b.path));
}

/** Only the app-owned export is marked. The source game is never modified. */
export async function markDeployment(root: string, id: string): Promise<DeploymentManifest> {
  if (!/^[a-zA-Z0-9-]+$/.test(id)) throw new Error(MESSAGE.InvalidIdentity);
  await tagEntry(root, id);
  const files = await exportFiles(root);
  if (!files.some((file) => file.path === ENTRY_FILE)) throw new Error(MESSAGE.NoEntryPage);
  const manifest = { id, digest: digest(JSON.stringify(files)), files };
  await writeFile(path.join(root, DEPLOYMENT_MARKER), JSON.stringify(manifest));
  return manifest;
}

/** The caller validates the server-issued origin. Every exported runtime file must match. */
export async function verifyDeployment(
  base: string,
  expected: DeploymentManifest,
  request: typeof fetch = fetch,
): Promise<void> {
  const deadline = AbortSignal.timeout(VERIFY_TIMEOUT_MS);
  const root = base.endsWith("/") ? base : `${base}/`;
  const read = async (relative: string) => {
    const url = new URL(relative.split("/").map(encodeURIComponent).join("/"), root);
    url.searchParams.set("_studio_check", `${expected.id}-${Date.now()}`);
    const response = await request(url, {
      cache: "no-store",
      redirect: "error",
      signal: AbortSignal.any([deadline, AbortSignal.timeout(FILE_READ_TIMEOUT_MS)]),
    });
    if (!response.ok) throw new Error(MESSAGE.ReadFailed(relative, response.status));
    return new Uint8Array(await response.arrayBuffer());
  };
  const marker = JSON.parse(new TextDecoder().decode(await read(DEPLOYMENT_MARKER))) as DeploymentManifest;
  if (marker.id !== expected.id || marker.digest !== expected.digest) throw new Error(MESSAGE.StillServingPrevious);
  // Bounded concurrency also covers static exports without hashed bundle names.
  for (let i = 0; i < expected.files.length; i += VERIFY_CONCURRENCY)
    await Promise.all(
      expected.files.slice(i, i + VERIFY_CONCURRENCY).map(async (file) => {
        const bytes = await read(file.path);
        // Genex's serving worker injects platform scripts into HTML. Verify our per-upload
        // entry marker there, and exact bytes for the local runtime dependencies.
        const matches =
          file.path === ENTRY_FILE
            ? new TextDecoder().decode(bytes).includes(deploymentTag(expected.id))
            : digest(bytes) === file.sha256;
        if (!matches) throw new Error(MESSAGE.NotReached(file.path));
      }),
    );
}
