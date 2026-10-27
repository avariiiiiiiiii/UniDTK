/** Root URL of the explorer package (…/explorer/), independent of the shell page URL. */
export const EXPLORER_ROOT = new URL("./", import.meta.url);

/** Resolve a path relative to the explorer folder (icons, etc.). */
export function explorerAsset(relPath) {
  const clean = String(relPath || "").replace(/^\//, "");
  return new URL(clean, EXPLORER_ROOT).href;
}
