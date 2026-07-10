// urlState.ts: deep links via the ?page= query param. Keeps the current page
// bookmarkable/shareable without a router; every other param (e.g. ?host=) is
// left untouched.
import type { Page } from './state';

const PAGES: readonly Page[] = ['entities', 'queries', 'stats', 'bsn', 'viewport', 'ecs'];

function isPage(value: string): value is Page {
  return (PAGES as readonly string[]).includes(value);
}

export function initialPage(): Page | null {
  const value = new URLSearchParams(location.search).get('page');
  if (value !== null && isPage(value)) return value;
  return null;
}

export function writePage(page: Page): void {
  const params = new URLSearchParams(location.search);
  params.set('page', page);
  const query = params.toString();
  history.replaceState(null, '', `${location.pathname}${query ? `?${query}` : ''}`);
}
