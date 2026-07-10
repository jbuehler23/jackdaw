// stats.ts: shared diagnostics poll, owned by the shell and reused by the Stats page.
import { jackdaw } from './brp';
import { createPoll } from './poll';

export const diagnosticsPoll = createPoll(() => jackdaw.diagnostics(), 1000);
