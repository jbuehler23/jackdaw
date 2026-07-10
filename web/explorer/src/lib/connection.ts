// connection.ts: handshake + liveness. Reads ?host= once at startup.
import { computed, signal } from '@preact/signals';
import { discoverCapabilities, jackdaw, setHost } from './brp';
import { createPoll } from './poll';

export type ConnectionStatus = 'connecting' | 'connected' | 'error';

const params = new URLSearchParams(location.search);
const hostParam = params.get('host');
if (hostParam) setHost(hostParam.includes(':') ? hostParam : `${hostParam}:15702`);

export const appInfo = signal<{ app_name: string; bevy_version: string } | null>(null);
export const capabilities = signal<Set<string>>(new Set());

export const connectionPoll = createPoll(async () => {
  const info = await jackdaw.appInfo();
  appInfo.value = info;
  return info;
}, 3000);

export const status = computed<ConnectionStatus>(() => {
  if (connectionPoll.error.value) return 'error';
  return appInfo.value ? 'connected' : 'connecting';
});

export async function startConnection() {
  connectionPoll.start();
  try {
    capabilities.value = await discoverCapabilities();
  } catch {
    // Plain BRP app: jackdaw methods absent, pages degrade individually.
  }
}
