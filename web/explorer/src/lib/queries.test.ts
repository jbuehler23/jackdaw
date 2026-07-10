import { describe, expect, it } from 'vitest';
import { buildCurl, buildQueryParams } from './queries';

describe('buildQueryParams', () => {
  it('maps fetch to data.option and filters', () => {
    expect(buildQueryParams(['a::B'], ['c::D'], ['e::F'])).toEqual({
      data: { option: ['a::B'] },
      filter: { with: ['c::D'], without: ['e::F'] },
    });
  });
});

describe('buildCurl', () => {
  it('produces a runnable world.query curl', () => {
    const cmd = buildCurl(buildQueryParams(['a::B'], [], []), 'localhost:15702');
    expect(cmd).toContain("curl -s -X POST http://localhost:15702/");
    expect(cmd).toContain('"method":"world.query"');
    expect(cmd).toContain('a::B');
  });
});
