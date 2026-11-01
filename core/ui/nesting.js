/**
 * UniDTK — nesting contract (nesting.js)
 *
 * A widget can live inside another one instead of on the desk. Two capabilities
 * decide it, both declared in a module's metadata.json:
 *   container   the widget hosts others (folders always do);
 *   embeddable  { module, render }: the file and the export that draw the
 *               compact form, which differs from the desk card.
 *
 * Nesting is one level deep: a container cannot be nested, so cycles cannot
 * exist. There is no limit on how many widgets a host takes.
 *
 * `config.nested` ({ hostId: [childId] }) is the user's intent and is never
 * trimmed: `effectiveNested` works out what applies now, so a module that is
 * off or a failed metadata read cannot lose a nesting for good.
 */

export function isContainer(widget) {
  return Boolean(widget?.isFolder || widget?.container);
}

/** Whether `child` may enter `host`: { ok: true } or { ok: false, reason }. */
export function canNest(child, host) {
  if (!child || !host) return { ok: false, reason: "Виджет не найден" };
  if (child.id === host.id) return { ok: false, reason: "Виджет нельзя вложить сам в себя" };
  if (!isContainer(host)) return { ok: false, reason: `«${host.name}» не может вмещать другие виджеты` };
  if (isContainer(child)) return { ok: false, reason: `«${child.name}» сам вмещает виджеты, такой нельзя вложить` };
  if (!child.embeddable) return { ok: false, reason: `У «${child.name}» нет компактного вида, вложить нельзя` };
  return { ok: true };
}

/**
 * The part of `stored` that applies to the widgets present now. A child whose
 * host is absent is left out, so it shows on the desk until the host returns.
 */
export function effectiveNested(stored, widgets) {
  const byId = new Map(widgets.map((w) => [w.id, w]));
  const effective = {};
  const taken = new Set();
  for (const [hostId, children] of Object.entries(stored || {})) {
    if (!Array.isArray(children)) continue;
    const host = byId.get(hostId);
    for (const childId of children) {
      if (taken.has(childId) || !canNest(byId.get(childId), host).ok) continue;
      taken.add(childId);
      (effective[hostId] ||= []).push(childId);
    }
  }
  return effective;
}

/** `stored` with `childId` under `hostId` only (or under nothing, when null). */
export function moveNested(stored, childId, hostId) {
  const next = {};
  for (const [id, children] of Object.entries(stored || {})) {
    const kept = (Array.isArray(children) ? children : []).filter((c) => c !== childId);
    if (kept.length) next[id] = kept;
  }
  if (hostId) (next[hostId] ||= []).push(childId);
  return next;
}
