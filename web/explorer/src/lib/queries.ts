// queries.ts: query-builder helpers shared by the Queries page and its curl export.
import type { QueryParams } from './brp';

export function buildQueryParams(fetchTypes: string[], withTypes: string[], withoutTypes: string[]): QueryParams {
  return {
    data: { option: fetchTypes },
    filter: { with: withTypes, without: withoutTypes },
  };
}

export function buildCurl(params: QueryParams, host: string): string {
  const body = { jsonrpc: '2.0', id: 1, method: 'world.query', params };
  const json = JSON.stringify(body).replace(/'/g, "'\\''");
  return `curl -s -X POST http://${host}/ -H 'Content-Type: application/json' -d '${json}'`;
}
