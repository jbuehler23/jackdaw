// QueriesPage.tsx: ad hoc world.query builder. Ported from the PoC's chipBox/
// runQuery/compPreview, with chips holding full type paths (the server needs
// full paths; only the display is shortened).
import { signal } from '@preact/signals';
import { useEffect, useRef, useState } from 'preact/hooks';
import { Copy, Play, SearchCode, X } from 'lucide-preact';
import { Icon } from './Icon';
import { currentHost, world, type QueryRow } from '../lib/brp';
import { buildCurl, buildQueryParams } from '../lib/queries';
import { loadRegistry, type ComponentSchema } from '../lib/registry';
import { createPoll } from '../lib/poll';
import { page, selectedEntity } from '../lib/state';
import { entityLabel, fmtNumber, shortTypeName } from '../lib/format';
import { toast } from '../lib/toasts';
import { NAME } from '../lib/tree';

export const fetchChips = signal<string[]>([]);
export const withChips = signal<string[]>([]);
const withoutChips = signal<string[]>([]);
const autoRefresh = signal(false);
const queryRows = signal<QueryRow[] | null>(null);
const queryNote = signal<string | null>(null);
const queryDurationMs = signal(0);

export async function runQuery(options: { silent?: boolean } = {}) {
  const fetchTypes = fetchChips.value;
  const withTypes = withChips.value;
  const withoutTypes = withoutChips.value;
  if (fetchTypes.length === 0 && withTypes.length === 0) {
    queryRows.value = null;
    queryNote.value = 'Add at least one component to fetch or filter on.';
    return;
  }
  // Always resolve Name too, so the results table can show it as a
  // convenience column even when it wasn't explicitly asked for.
  const optionTypes = fetchTypes.includes(NAME) ? fetchTypes : [...fetchTypes, NAME];
  const params = buildQueryParams(optionTypes, withTypes, withoutTypes);
  const t0 = performance.now();
  try {
    const rows = await world.query(params);
    queryRows.value = rows;
    queryDurationMs.value = performance.now() - t0;
    queryNote.value = null;
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    // The auto-refresh poll ticks every 2s; toasting on every failed tick would
    // spam the user when the server dies with auto-refresh left on. Route poll
    // failures into the same inline note used for other empty-state messages
    // instead, and reserve the toast for a manual Run click.
    if (options.silent) queryNote.value = message;
    else toast('err', message);
  }
}

const queryPoll = createPoll(() => runQuery({ silent: true }), 2000);

function compPreview(value: unknown, schema: ComponentSchema | undefined): string {
  if (value === undefined) return '';
  if (schema && schema.fields === 'marker') return '()';
  if (typeof value === 'string') return value;
  if (value && typeof value === 'object' && 'translation' in value) {
    const t = (value as { translation: unknown }).translation;
    if (t && typeof t === 'object' && 'x' in t && 'y' in t && 'z' in t) {
      const v = t as { x: number; y: number; z: number };
      return `t: (${fmtNumber(v.x)}, ${fmtNumber(v.y)}, ${fmtNumber(v.z)})`;
    }
  }
  return JSON.stringify(value).slice(0, 60);
}

/** Seeds the query builder's chips (e.g. from an archetype row's "query"
 * button) and runs the query. Clears the without-filter so a stale
 * exclusion from a prior manual query doesn't hide the seeded result. */
export function seedQuery(fetch: string[], withList: string[]) {
  fetchChips.value = fetch;
  withChips.value = withList;
  withoutChips.value = [];
  void runQuery();
}

function jumpToEntity(entity: number) {
  selectedEntity.value = entity;
  page.value = 'entities';
}

function ChipBox({
  items,
  onAdd,
  onRemove,
  placeholder,
  registry,
}: {
  items: string[];
  onAdd: (typePath: string) => void;
  onRemove: (index: number) => void;
  placeholder: string;
  registry: Map<string, ComponentSchema> | null;
}) {
  const [text, setText] = useState('');
  const [open, setOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const matches = registry
    ? Array.from(registry.keys())
        .filter((k) => !items.includes(k) && k.toLowerCase().includes(text.toLowerCase()))
        .slice(0, 8)
    : [];

  function add(typePath: string) {
    onAdd(typePath);
    setText('');
    setOpen(false);
    inputRef.current?.focus();
  }

  return (
    <div class="chip-box">
      {items.map((typePath, i) => (
        <span class="chip" key={typePath} title={typePath}>
          {registry?.get(typePath)?.shortName ?? shortTypeName(typePath)}
          <button onClick={() => onRemove(i)}>
            <Icon of={X} />
          </button>
        </span>
      ))}
      <input
        ref={inputRef}
        placeholder={placeholder}
        spellcheck={false}
        value={text}
        onInput={(ev) => {
          setText((ev.target as HTMLInputElement).value);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 150)}
        onKeyDown={(ev) => {
          if (ev.key === 'Enter') {
            if (matches[0]) add(matches[0]);
          } else if (ev.key === 'Backspace' && !text && items.length) {
            onRemove(items.length - 1);
          } else if (ev.key === 'Escape') {
            setOpen(false);
          }
        }}
      />
      {open && matches.length > 0 && (
        <div class="autocomplete">
          {matches.map((typePath, i) => (
            <button
              class={`ac-item${i === 0 ? ' hot' : ''}`}
              key={typePath}
              onMouseDown={(ev) => {
                ev.preventDefault();
                add(typePath);
              }}
            >
              {registry?.get(typePath)?.shortName ?? shortTypeName(typePath)}{' '}
              <span style="color:var(--text-secondary)">· {typePath}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export function QueriesPage() {
  const [registry, setRegistry] = useState<Map<string, ComponentSchema> | null>(null);

  useEffect(() => {
    loadRegistry()
      .then(setRegistry)
      .catch(() => {
        // Autocomplete falls back to free-typed chips (still matched against
        // full type paths server-side).
      });
  }, []);

  useEffect(() => {
    if (autoRefresh.value) queryPoll.start();
    else queryPoll.stop();
    return () => queryPoll.stop();
  }, [autoRefresh.value]);

  function copyCurl() {
    const optionTypes = fetchChips.value.includes(NAME) ? fetchChips.value : [...fetchChips.value, NAME];
    const params = buildQueryParams(optionTypes, withChips.value, withoutChips.value);
    const cmd = buildCurl(params, currentHost());
    if (!navigator.clipboard) {
      toast('err', 'Clipboard unavailable in this context');
      return;
    }
    navigator.clipboard.writeText(cmd).then(
      () => toast('ok', 'Copied world.query curl command'),
      () => toast('err', 'Clipboard unavailable in this context'),
    );
  }

  const rows = queryRows.value;

  return (
    <div class="pane" style="flex:1">
      <div class="pane-header">
        <Icon of={SearchCode} />
        Query
      </div>
      <div class="query-wrap">
        <div class="query-builder">
          <div class="qb-row">
            <span class="qb-label">Fetch</span>
            <ChipBox
              items={fetchChips.value}
              onAdd={(t) => {
                fetchChips.value = [...fetchChips.value, t];
                if (autoRefresh.value) void runQuery();
              }}
              onRemove={(i) => {
                fetchChips.value = fetchChips.value.filter((_, idx) => idx !== i);
                if (autoRefresh.value) void runQuery();
              }}
              placeholder="component to fetch"
              registry={registry}
            />
          </div>
          <div class="qb-row">
            <span class="qb-label">With</span>
            <ChipBox
              items={withChips.value}
              onAdd={(t) => {
                withChips.value = [...withChips.value, t];
                if (autoRefresh.value) void runQuery();
              }}
              onRemove={(i) => {
                withChips.value = withChips.value.filter((_, idx) => idx !== i);
                if (autoRefresh.value) void runQuery();
              }}
              placeholder="must have"
              registry={registry}
            />
          </div>
          <div class="qb-row">
            <span class="qb-label">Without</span>
            <ChipBox
              items={withoutChips.value}
              onAdd={(t) => {
                withoutChips.value = [...withoutChips.value, t];
                if (autoRefresh.value) void runQuery();
              }}
              onRemove={(i) => {
                withoutChips.value = withoutChips.value.filter((_, idx) => idx !== i);
                if (autoRefresh.value) void runQuery();
              }}
              placeholder="must not have"
              registry={registry}
            />
          </div>
          <div class="qb-actions">
            <button class="btn-primary" onClick={() => void runQuery()}>
              <Icon of={Play} />
              Run query
            </button>
            <label class="check-label">
              <input
                type="checkbox"
                checked={autoRefresh.value}
                onChange={(ev) => {
                  autoRefresh.value = (ev.target as HTMLInputElement).checked;
                }}
              />
              Auto-refresh
            </label>
            <button class="btn-ghost" onClick={copyCurl}>
              <Icon of={Copy} />
              Copy as curl
            </button>
            {rows && (
              <span class="results-meta">
                {rows.length} {rows.length === 1 ? 'row' : 'rows'} · {queryDurationMs.value.toFixed(1)} ms
              </span>
            )}
          </div>
        </div>
        <div class="results-scroll">
          {rows === null ? (
            <div class="query-empty">
              <div class="icon">
                <Icon of={SearchCode} />
              </div>
              <div>
                {queryNote.value ?? (
                  <>
                    Add components to fetch, then run the query.
                    <br />
                    Runs <code style="font-family:var(--font-mono)">world.query</code> against the connected app.
                  </>
                )}
              </div>
            </div>
          ) : (
            <table class="results">
              <thead>
                <tr>
                  <th>Entity</th>
                  <th>Name</th>
                  {fetchChips.value.map((t) => (
                    <th key={t}>{registry?.get(t)?.shortName ?? shortTypeName(t)}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => {
                  const name = typeof row.components[NAME] === 'string' ? (row.components[NAME] as string) : null;
                  return (
                    <tr key={row.entity}>
                      <td style="font-family:var(--font-mono)">{entityLabel(row.entity)}</td>
                      <td>
                        <button class="entity-link" onClick={() => jumpToEntity(row.entity)}>
                          {name ?? 'Entity'}
                        </button>
                      </td>
                      {fetchChips.value.map((t) => {
                        const preview = compPreview(row.components[t], registry?.get(t));
                        return (
                          <td key={t} title={preview}>
                            {preview}
                          </td>
                        );
                      })}
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </div>
      </div>
    </div>
  );
}
