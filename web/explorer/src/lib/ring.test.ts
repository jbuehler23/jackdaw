import { describe, expect, it } from 'vitest';
import { RingBuffer } from './ring';

describe('RingBuffer', () => {
  it('keeps the last N values in order', () => {
    const r = new RingBuffer(3);
    [1, 2, 3, 4].forEach((v) => r.push(v));
    expect(r.values()).toEqual([2, 3, 4]);
  });
});
