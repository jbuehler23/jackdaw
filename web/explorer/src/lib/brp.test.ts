import { afterEach, describe, expect, it, vi } from 'vitest';
import { BrpError, brpCall, discoverCapabilities, jackdaw, setHost, world } from './brp';

function mockFetchOnce(body: unknown) {
  const fn = vi.fn().mockResolvedValue({ ok: true, json: () => Promise.resolve(body) });
  vi.stubGlobal('fetch', fn);
  return fn;
}

afterEach(() => vi.unstubAllGlobals());

describe('brpCall', () => {
  it('posts JSON-RPC and returns result', async () => {
    const fetchFn = mockFetchOnce({ jsonrpc: '2.0', id: 1, result: { ok: true } });
    setHost('localhost:15702');
    const result = await brpCall('jackdaw/app_info');
    expect(result).toEqual({ ok: true });
    const [url, init] = fetchFn.mock.calls[0];
    expect(url).toBe('http://localhost:15702/');
    const sent = JSON.parse((init as RequestInit).body as string);
    expect(sent.method).toBe('jackdaw/app_info');
    expect(sent.jsonrpc).toBe('2.0');
  });

  it('throws BrpError with code on JSON-RPC error', async () => {
    mockFetchOnce({ jsonrpc: '2.0', id: 1, error: { code: -32602, message: 'bad params' } });
    await expect(brpCall('world.query', {})).rejects.toMatchObject({ code: -32602, message: 'bad params' });
    await expect(brpCall('world.query', {})).rejects.toBeInstanceOf(BrpError);
  });
});

describe('typed wrappers', () => {
  it('world.query passes params through', async () => {
    const fetchFn = mockFetchOnce({ jsonrpc: '2.0', id: 1, result: [{ entity: 42, components: {} }] });
    const rows = await world.query({ data: { option: ['a::B'] } });
    expect(rows[0].entity).toBe(42);
    const sent = JSON.parse((fetchFn.mock.calls[0][1] as RequestInit).body as string);
    expect(sent.params.data.option).toEqual(['a::B']);
  });

  it('mutateComponents shapes the request', async () => {
    const fetchFn = mockFetchOnce({ jsonrpc: '2.0', id: 1, result: null });
    await world.mutateComponents(7, 'a::B', 'translation.x', 1.5);
    const sent = JSON.parse((fetchFn.mock.calls[0][1] as RequestInit).body as string);
    expect(sent.method).toBe('world.mutate_components');
    expect(sent.params).toEqual({ entity: 7, component: 'a::B', path: 'translation.x', value: 1.5 });
  });

  it('jackdaw.playback and applyBsn shape requests', async () => {
    let fetchFn = mockFetchOnce({ jsonrpc: '2.0', id: 1, result: { paused: true } });
    await jackdaw.playback('pause');
    let sent = JSON.parse((fetchFn.mock.calls[0][1] as RequestInit).body as string);
    expect(sent).toMatchObject({ method: 'jackdaw/playback', params: { action: 'pause' } });

    fetchFn = mockFetchOnce({ jsonrpc: '2.0', id: 1, result: { entities: [1] } });
    await jackdaw.applyBsn('#X');
    sent = JSON.parse((fetchFn.mock.calls[0][1] as RequestInit).body as string);
    expect(sent).toMatchObject({ method: 'jackdaw/apply_bsn', params: { source: '#X' } });
  });
});

describe('discoverCapabilities', () => {
  it('returns the method-name set', async () => {
    mockFetchOnce({ jsonrpc: '2.0', id: 1, result: { methods: [{ name: 'jackdaw/diagnostics' }, { name: 'world.query' }] } });
    const caps = await discoverCapabilities();
    expect(caps.has('jackdaw/diagnostics')).toBe(true);
    expect(caps.has('jackdaw/nope')).toBe(false);
  });
});
