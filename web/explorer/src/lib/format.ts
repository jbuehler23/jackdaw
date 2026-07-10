// format.ts: display formatting helpers shared across pages.
export function fmtNumber(n: number): string {
  if (Number.isInteger(n)) return n.toFixed(1);
  return String(Math.round(n * 1000) / 1000);
}
export function shortTypeName(typePath: string): string {
  const base = typePath.split('<')[0];
  const short = base.split('::').pop() ?? typePath;
  return typePath.includes('<') ? `${short}<${typePath.split('<').slice(1).join('<')}` : short;
}
export function entityLabel(bits: number): string {
  // Exact only while bits < 2^53; generations beyond ~2^21 at max index would blend.
  // Matches the server-side display: low 32 bits index, high 32 generation.
  const index = bits >>> 0;
  const generation = Math.floor(bits / 2 ** 32);
  return `${index}v${generation}`;
}
