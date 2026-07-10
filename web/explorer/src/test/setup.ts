// setup.ts: unmounts rendered components between tests so DOM queries don't
// see stale nodes from a previous test's render.
import { cleanup } from '@testing-library/preact';
import { afterEach } from 'vitest';

afterEach(() => {
  cleanup();
});
