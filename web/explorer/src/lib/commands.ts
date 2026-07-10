// commands.ts: shell actions shared between the topbar and the command
// palette, kept out of app.tsx so the palette doesn't import the shell module.
import { jackdaw } from './brp';
import { pollingPaused, simPaused } from './state';
import { toast } from './toasts';

export async function toggleSim() {
  const action = simPaused.value ? 'resume' : 'pause';
  try {
    const result = await jackdaw.playback(action);
    simPaused.value = result.paused;
    toast('info', `jackdaw/playback: ${action}`);
  } catch (err) {
    toast('err', `jackdaw/playback failed: ${err instanceof Error ? err.message : String(err)}`);
  }
}

export async function stepSim() {
  try {
    await jackdaw.playback('step');
    toast('info', 'jackdaw/playback: step');
  } catch (err) {
    toast('err', `jackdaw/playback failed: ${err instanceof Error ? err.message : String(err)}`);
  }
}

export function togglePolling() {
  pollingPaused.value = !pollingPaused.value;
  toast('info', pollingPaused.value ? 'Polling paused; the game keeps running' : 'Polling resumed');
}
