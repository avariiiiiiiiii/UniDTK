/**
 * UniDTK — real shortcut icons (shortcut-icons.js)
 *
 * The kernel extracts the icon Windows itself shows for a file, program or
 * folder (`shortcut_icon`, BGRA pixels; there is no PNG encoder in the core)
 * and caches it on disk. Here the pixels become a data URL once per target.
 */

import { invoke } from "@unidtk/bridge";

const ICON_PX = 48;
const cache = new Map();

function toDataUrl({ width, height, bgra }) {
  const bytes = Uint8ClampedArray.from(atob(bgra), (c) => c.charCodeAt(0));
  // BGRA → RGBA in place.
  for (let i = 0; i < bytes.length; i += 4) {
    const b = bytes[i];
    bytes[i] = bytes[i + 2];
    bytes[i + 2] = b;
  }
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  canvas.getContext("2d").putImageData(new ImageData(bytes, width, height), 0, 0);
  return canvas.toDataURL();
}

/** Data URL of the target's Windows icon, or "" when there is none (URLs, missing files). */
export function shortcutIconUrl(target) {
  const key = String(target || "");
  if (!key || /^[a-z][a-z0-9+.-]*:\/\//i.test(key)) return Promise.resolve("");
  if (!cache.has(key)) {
    cache.set(key, invoke("shortcut_icon", { target: key, size: ICON_PX })
      .then((icon) => (icon ? toDataUrl(icon) : ""))
      .catch(() => ""));
  }
  return cache.get(key);
}

/** Fill every `img[data-icon-target]` under `root` once its icon arrives. */
export function hydrateShortcutIcons(root) {
  root.querySelectorAll("img[data-icon-target]").forEach((img) => {
    shortcutIconUrl(img.dataset.iconTarget).then((url) => {
      if (!url || !img.isConnected) return;
      img.src = url;
      img.hidden = false;
      img.nextElementSibling?.classList.add("has-real-icon");
    });
  });
}
