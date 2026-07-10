// Icon.tsx: thin wrapper fixing the PoC's .icon sizing convention.
import type { LucideIcon } from 'lucide-preact';

export function Icon({ of: Of, class: cls }: { of: LucideIcon; class?: string }) {
  return (
    <span class={`icon ${cls ?? ''}`}>
      <Of />
    </span>
  );
}
