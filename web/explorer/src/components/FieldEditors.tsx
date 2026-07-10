// FieldEditors.tsx: the leaf controls the inspector's field rows are built from.
// Every editor takes the field's current value and a binding (which component/path
// it writes to) and calls onCommit(path, value); Inspector.tsx owns turning that
// into the actual mutate_components/insert_components call.
import { useEffect, useRef } from 'preact/hooks';
import { Link } from 'lucide-preact';
import { Icon } from './Icon';
import type { FieldBinding } from '../lib/inspector';
import { commitPath, scrubValue } from '../lib/inspector';
import { fmtNumber, entityLabel } from '../lib/format';
import { selectedEntity } from '../lib/state';

type Commit = (path: string, value: unknown) => void;

const DRAG_THRESHOLD_PX = 3;

interface Scrub {
  startX: number;
  start: number;
  moved: boolean;
}

export function NumberCell({
  binding,
  value,
  axis,
  step,
  onCommit,
  onCommitOverride,
}: {
  binding: FieldBinding;
  value: number;
  axis?: 'x' | 'y' | 'z' | 'w' | null;
  step: number;
  onCommit: Commit;
  // Whole-value vec fields (single-field tuple structs unwrapped to a bare vec, e.g.
  // avian's Position(Vec3)) have no dotted reflection path to commit an axis through:
  // the parent binding's path is '' and there's no addressable "path.x" on the wire.
  // When set, this replaces the normal onCommit(binding.path, value) call.
  onCommitOverride?: (value: number) => void;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const scrubRef = useRef<Scrub | null>(null);
  const scrubbingRef = useRef(false);

  // Live poll updates must not clobber an input the user is actively scrubbing or
  // has focused for typed entry.
  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    if (scrubbingRef.current || document.activeElement === el) return;
    el.value = fmtNumber(value);
  }, [value]);

  function revertOrCommit() {
    const el = inputRef.current;
    if (!el) return;
    const parsed = parseFloat(el.value);
    if (Number.isNaN(parsed)) {
      el.value = fmtNumber(value);
      return;
    }
    el.value = fmtNumber(parsed);
    if (onCommitOverride) onCommitOverride(parsed);
    else onCommit(binding.path, parsed);
  }

  return (
    <span class={`num-cell${axis ? '' : ' wide'}`}>
      {axis && <span class="axis" style={`background:var(--axis-${axis})`} />}
      <input
        ref={inputRef}
        defaultValue={fmtNumber(value)}
        spellcheck={false}
        onPointerDown={(ev) => {
          const el = inputRef.current;
          if (!el || document.activeElement === el) return; // already focused: typing mode
          scrubRef.current = { startX: ev.clientX, start: parseFloat(el.value) || 0, moved: false };
          el.setPointerCapture(ev.pointerId);
        }}
        onPointerMove={(ev) => {
          const scrub = scrubRef.current;
          const el = inputRef.current;
          if (!scrub || !el) return;
          const dx = ev.clientX - scrub.startX;
          if (!scrub.moved && Math.abs(dx) < DRAG_THRESHOLD_PX) return;
          if (!scrub.moved) {
            scrub.moved = true;
            scrubbingRef.current = true;
            el.parentElement?.classList.add('scrubbing');
          }
          el.value = fmtNumber(scrubValue(scrub.start, dx, step, ev.shiftKey));
        }}
        onPointerUp={(ev) => {
          const el = inputRef.current;
          const scrub = scrubRef.current;
          el?.releasePointerCapture(ev.pointerId);
          el?.parentElement?.classList.remove('scrubbing');
          if (scrub && scrub.moved && el) {
            scrubbingRef.current = false;
            const parsed = parseFloat(el.value);
            if (onCommitOverride) onCommitOverride(parsed);
            else onCommit(binding.path, parsed);
          } else if (scrub && el) {
            el.focus();
            el.select();
          }
          scrubRef.current = null;
        }}
        onKeyDown={(ev) => {
          if (ev.key === 'Enter') (ev.target as HTMLInputElement).blur();
        }}
        onBlur={revertOrCommit}
      />
    </span>
  );
}

const AXES3 = ['x', 'y', 'z'] as const;
const AXES4 = ['x', 'y', 'z', 'w'] as const;

export function VecRow({
  binding,
  value,
  onCommit,
}: {
  binding: FieldBinding;
  value: Record<string, number> | number[];
  onCommit: Commit;
}) {
  const axes = binding.kind === 'vec4' || binding.kind === 'quat' ? AXES4 : AXES3;
  const step = binding.kind === 'quat' ? 0.01 : 0.1;
  const at = (axis: string, index: number): number =>
    Array.isArray(value) ? (value[index] as number) : (value as Record<string, number>)[axis];
  // A single-field tuple struct (e.g. avian's Position(Vec3)) unwraps to binding.path
  // === ''; there's no dotted reflection path an axis edit could mutate through, so
  // each axis commit has to replace the whole component value instead.
  const isWholeValue = binding.path === '';

  return (
    <>
      {axes.map((axis, index) => (
        <NumberCell
          key={axis}
          binding={{ component: binding.component, path: isWholeValue ? axis : `${binding.path}.${axis}`, kind: 'f32' }}
          value={at(axis, index)}
          axis={axis}
          step={step}
          onCommit={onCommit}
          onCommitOverride={isWholeValue ? (next) => onCommit('', commitPath(value, axis, next)) : undefined}
        />
      ))}
      {binding.kind === 'vec3' && <span class="spacer" />}
    </>
  );
}

export function BoolSwitch({ binding, value, onCommit }: { binding: FieldBinding; value: boolean; onCommit: Commit }) {
  return (
    <span
      class={`switch${value ? ' on' : ''}`}
      role="switch"
      aria-checked={value}
      tabIndex={0}
      onClick={() => onCommit(binding.path, !value)}
      onKeyDown={(ev) => {
        if (ev.key === 'Enter' || ev.key === ' ') {
          ev.preventDefault();
          onCommit(binding.path, !value);
        }
      }}
    />
  );
}

export function EnumSelect({
  binding,
  value,
  options,
  onCommit,
}: {
  binding: FieldBinding;
  value: string;
  options: string[];
  onCommit: Commit;
}) {
  return (
    <select
      class="txt-cell"
      value={value}
      onChange={(ev) => onCommit(binding.path, (ev.target as HTMLSelectElement).value)}
    >
      {options.map((option) => (
        <option key={option} value={option}>
          {option}
        </option>
      ))}
    </select>
  );
}

export function StringInput({ binding, value, onCommit }: { binding: FieldBinding; value: string; onCommit: Commit }) {
  return (
    <input
      class="txt-cell"
      style="font-family:var(--font-mono);font-size:10px"
      defaultValue={value}
      spellcheck={false}
      onBlur={(ev) => onCommit(binding.path, (ev.target as HTMLInputElement).value)}
      onKeyDown={(ev) => {
        if (ev.key === 'Enter') (ev.target as HTMLInputElement).blur();
      }}
    />
  );
}

export function EntityLink({ value, label }: { value: number; label?: string }) {
  return (
    <button
      class="entity-link"
      onClick={() => {
        selectedEntity.value = value;
      }}
    >
      <Icon of={Link} />
      {label ?? 'Entity'} <span style="color:var(--text-secondary)">{entityLabel(value)}</span>
    </button>
  );
}

// Full color editing is out of scope for v1; shows a swatch when the value looks
// like an sRGBA color object plus the raw value as read-only JSON.
export function ColorField({ value }: { value: unknown }) {
  const srgba = (value as { Srgba?: { red: number; green: number; blue: number } })?.Srgba;
  const swatchColor = srgba
    ? `rgb(${Math.round(srgba.red * 255)},${Math.round(srgba.green * 255)},${Math.round(srgba.blue * 255)})`
    : 'transparent';
  return (
    <>
      <span class="color-swatch" style={`background:${swatchColor}`} />
      <span class="txt-cell" style="font-family:var(--font-mono);font-size:10px">
        {JSON.stringify(value)}
      </span>
    </>
  );
}

export function OpaqueJson({ value }: { value: unknown }) {
  return (
    <div class="marker-note">
      <pre style="white-space:pre-wrap;font-family:var(--font-mono)">{JSON.stringify(value, null, 2)}</pre>
    </div>
  );
}
