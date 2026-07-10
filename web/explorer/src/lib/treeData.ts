// treeData.ts: the entity tree poll, shared by every page that needs tree
// rows (TreePanel, Inspector, BsnPage, CommandPalette) without importing the
// TreePanel component module.
import { fetchTreeRows } from './tree';
import { createPoll } from './poll';

export const treePoll = createPoll(fetchTreeRows, 2000);

export { assembleTree } from './tree';
