// BRP JSON-RPC client. The only module that knows the wire format.

export class BrpError extends Error {
  code: number;
  constructor(code: number, message: string) {
    super(message);
    this.code = code;
  }
}

let host = 'localhost:15702';
let nextId = 1;

export function setHost(newHost: string) {
  host = newHost;
}

export function currentHost(): string {
  return host;
}

interface RpcResponse {
  result?: unknown;
  error?: { code: number; message: string };
}

export async function brpCall<T = unknown>(method: string, params?: unknown): Promise<T> {
  const response = await fetch(`http://${host}/`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: nextId++, method, ...(params !== undefined ? { params } : {}) }),
  });
  const body = (await response.json()) as RpcResponse;
  if (body.error) throw new BrpError(body.error.code, body.error.message);
  return body.result as T;
}

export interface QueryParams {
  data?: { option?: string[]; components?: string[]; has?: string[] };
  filter?: { with?: string[]; without?: string[] };
}
export interface QueryRow {
  entity: number;
  components: Record<string, unknown>;
  has?: Record<string, boolean>;
}

export const world = {
  query: (params: QueryParams) => brpCall<QueryRow[]>('world.query', params),
  listComponents: (entity: number) => brpCall<string[]>('world.list_components', { entity }),
  getComponents: (entity: number, components: string[]) =>
    brpCall<{ components: Record<string, unknown> }>('world.get_components', { entity, components, strict: false })
      .then((r) => r.components ?? (r as unknown as Record<string, unknown>)),
  mutateComponents: (entity: number, component: string, path: string, value: unknown) =>
    brpCall<void>('world.mutate_components', { entity, component, path, value }),
  insertComponents: (entity: number, components: Record<string, unknown>) =>
    brpCall<void>('world.insert_components', { entity, components }),
  removeComponents: (entity: number, components: string[]) =>
    brpCall<void>('world.remove_components', { entity, components }),
  spawnEntity: (components: Record<string, unknown>) =>
    brpCall<{ entity: number }>('world.spawn_entity', { components }).then((r) => r.entity),
  despawnEntity: (entity: number) => brpCall<void>('world.despawn_entity', { entity }),
  reparentEntities: (entities: number[], parent: number | null) =>
    brpCall<void>('world.reparent_entities', parent === null ? { entities } : { entities, parent }),
};

export interface ScheduleSystem {
  name: string;
  sets: string[];
}
export interface ScheduleInfo {
  schedule: string;
  initialized: boolean;
  systems: ScheduleSystem[];
  edges: [number, number][];
}
export interface ArchetypeInfo {
  components: string[];
  entity_count: number;
  bytes_per_entity: number;
}

// Raw wire shape for `jackdaw/schedules`: older servers send `systems` as a
// plain array of names with no `edges`; the enriched shape sends
// `{name, sets}` entries plus a run-order-indexed `edges` list. Both are
// normalized to ScheduleInfo below so callers never branch on server version.
interface RawScheduleSystem {
  name?: string;
  sets?: string[];
}
interface RawScheduleInfo {
  schedule: string;
  initialized: boolean;
  systems: (string | RawScheduleSystem)[];
  edges?: [number, number][];
}

function normalizeScheduleSystems(systems: (string | RawScheduleSystem)[]): ScheduleSystem[] {
  return systems.map((system) =>
    typeof system === 'string' ? { name: system, sets: [] } : { name: system.name ?? '', sets: system.sets ?? [] },
  );
}

export const jackdaw = {
  appInfo: () => brpCall<{ app_name: string; bevy_version: string }>('jackdaw/app_info'),
  diagnostics: () =>
    brpCall<{ fps: number | null; frame_time_ms: number | null; entity_count: number }>('jackdaw/diagnostics'),
  playback: (action: 'pause' | 'resume' | 'step') => brpCall<{ paused: boolean }>('jackdaw/playback', { action }),
  applyBsn: (source: string) => brpCall<{ entities: number[] }>('jackdaw/apply_bsn', { source }),
  entityBsn: (entity: number) => brpCall<{ bsn: string }>('jackdaw/entity_bsn', { entity }),
  schedules: async (): Promise<{ schedules: ScheduleInfo[] }> => {
    const raw = await brpCall<{ schedules: RawScheduleInfo[] }>('jackdaw/schedules');
    return {
      schedules: raw.schedules.map((s) => ({
        schedule: s.schedule,
        initialized: s.initialized,
        systems: normalizeScheduleSystems(s.systems),
        edges: s.edges ?? [],
      })),
    };
  },
  archetypes: () => brpCall<{ archetypes: ArchetypeInfo[] }>('jackdaw/archetypes'),
};

export async function discoverCapabilities(): Promise<Set<string>> {
  const result = await brpCall<{ methods: { name: string }[] }>('rpc.discover');
  return new Set(result.methods.map((m) => m.name));
}
