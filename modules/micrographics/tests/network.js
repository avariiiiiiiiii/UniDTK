(async () => {
  const { buildTokens } = await import("/modules/micrographics/composition.js");
  const now = new Date();
  const seconds = Math.floor(now.getTime() / 1000);
  let checks = 0;
  const assert = (condition, message) => {
    checks++;
    if (!condition) throw new Error(message);
  };
  const payload = { ipLocation: { ip: "8.8.8.8", city: "Example", country: "Example",
    resolvedAt: seconds, stale: false } };
  const label = weather => buildTokens(now, { weather }).find(token => token.id === "ip:source")?.v;
  const before = JSON.stringify(payload);
  assert(label(payload) === "IP", "fresh IP shown as fresh");
  assert(label({ ipLocation: { ...payload.ipLocation, resolvedAt: seconds - 1801 } }) === "IP · УСТАРЕЛ",
    "IP becomes stale without another backend event");
  assert(label({ ipLocation: { ...payload.ipLocation, stale: true } }) === "IP · УСТАРЕЛ",
    "route invalidation overrides a recent timestamp");
  assert(JSON.stringify(payload) === before, "display does not alter the backend payload");
  return { ok: true, checks };
})()
