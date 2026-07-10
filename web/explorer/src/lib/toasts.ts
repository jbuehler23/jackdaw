// toasts.ts: transient notifications for mutating actions, mirrors the PoC's toast() helper.
import { signal } from '@preact/signals';

export type ToastKind = 'ok' | 'err' | 'info';
export interface ToastItem {
  id: number;
  kind: ToastKind;
  message: string;
}

export const toasts = signal<ToastItem[]>([]);

let nextId = 1;

export function toast(kind: ToastKind, message: string) {
  const id = nextId++;
  toasts.value = [...toasts.value, { id, kind, message }];
  setTimeout(() => {
    toasts.value = toasts.value.filter((t) => t.id !== id);
  }, 3200);
}
