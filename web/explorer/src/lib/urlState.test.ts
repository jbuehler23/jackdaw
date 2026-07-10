import { beforeEach, describe, expect, it } from 'vitest';
import { initialPage, writePage } from './urlState';

function setSearch(search: string) {
  history.replaceState(null, '', `${location.pathname}${search}`);
}

beforeEach(() => {
  setSearch('');
});

describe('initialPage', () => {
  it('reads a valid page from ?page=', () => {
    setSearch('?page=viewport');
    expect(initialPage()).toBe('viewport');
  });

  it('returns null when ?page= is missing', () => {
    setSearch('');
    expect(initialPage()).toBeNull();
  });

  it('returns null for an invalid page value', () => {
    setSearch('?page=bogus');
    expect(initialPage()).toBeNull();
  });
});

describe('writePage', () => {
  it('sets ?page= without reloading', () => {
    setSearch('');
    writePage('stats');
    expect(new URLSearchParams(location.search).get('page')).toBe('stats');
  });

  it('preserves other params like ?host=', () => {
    setSearch('?host=localhost:1234');
    writePage('bsn');
    const params = new URLSearchParams(location.search);
    expect(params.get('host')).toBe('localhost:1234');
    expect(params.get('page')).toBe('bsn');
  });

  it('overwrites an existing ?page= value', () => {
    setSearch('?page=entities&host=localhost:1234');
    writePage('queries');
    const params = new URLSearchParams(location.search);
    expect(params.get('page')).toBe('queries');
    expect(params.get('host')).toBe('localhost:1234');
  });
});
