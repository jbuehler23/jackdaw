import { render } from '@testing-library/preact';
import { fireEvent } from '@testing-library/preact';
import { describe, expect, it, vi } from 'vitest';
import { EnumDataField, OpaqueJson, VecRow } from '../components/FieldEditors';
import type { EnumDataVariant } from '../lib/registry';

const LINE_HEIGHT_VARIANTS: EnumDataVariant[] = [
  { name: 'Px', payload: 'f32' },
  { name: 'RelativeToFont', payload: 'f32' },
];

describe('EnumDataField', () => {
  const binding = { component: 'bevy_text::text::LineHeight', path: '', kind: 'enumdata' as const };

  it('renders the current variant and its payload', () => {
    const onCommit = vi.fn();
    const { container } = render(
      <EnumDataField binding={binding} value={{ RelativeToFont: 1.2 }} variants={LINE_HEIGHT_VARIANTS} onCommit={onCommit} />,
    );
    const select = container.querySelector('select') as HTMLSelectElement;
    expect(select.value).toBe('RelativeToFont');
    const input = container.querySelector('input') as HTMLInputElement;
    expect(input.value).toBe('1.2');
  });

  it('switching the variant commits {NewVariant: 0}', () => {
    const onCommit = vi.fn();
    const { container } = render(
      <EnumDataField binding={binding} value={{ Px: 0 }} variants={LINE_HEIGHT_VARIANTS} onCommit={onCommit} />,
    );
    const select = container.querySelector('select') as HTMLSelectElement;
    fireEvent.change(select, { target: { value: 'RelativeToFont' } });
    expect(onCommit).toHaveBeenCalledWith('', { RelativeToFont: 0 });
  });

  it('editing the payload commits {Variant: newValue}', () => {
    const onCommit = vi.fn();
    const { container } = render(
      <EnumDataField binding={binding} value={{ Px: 0 }} variants={LINE_HEIGHT_VARIANTS} onCommit={onCommit} />,
    );
    const input = container.querySelector('input') as HTMLInputElement;
    input.value = '5';
    fireEvent.blur(input);
    expect(onCommit).toHaveBeenCalledWith('', { Px: 5 });
  });

  it('renders a bare-string commit for a unit variant', () => {
    const onCommit = vi.fn();
    const variants: EnumDataVariant[] = [
      { name: 'Normal', payload: 'none' },
      { name: 'Px', payload: 'f32' },
    ];
    const { container } = render(
      <EnumDataField binding={binding} value="Normal" variants={variants} onCommit={onCommit} />,
    );
    const select = container.querySelector('select') as HTMLSelectElement;
    expect(select.value).toBe('Normal');
    expect(container.querySelector('input')).toBeNull();
    fireEvent.change(select, { target: { value: 'Px' } });
    expect(onCommit).toHaveBeenCalledWith('', { Px: 0 });
  });
});

describe('VecRow', () => {
  it('renders an array-shaped vec2 value from indices, not dotted x/y lookups', () => {
    const binding = { component: 'bevy_ui::ui_node::ComputedNode', path: 'content_size', kind: 'vec2' as const };
    const { container } = render(<VecRow binding={binding} value={[360, 48]} onCommit={vi.fn()} />);
    const cells = container.querySelectorAll<HTMLElement>('.num-cell');
    expect(cells.length).toBe(2);
    expect((cells[0].querySelector('input') as HTMLInputElement).value).toBe('360.0');
    expect((cells[1].querySelector('input') as HTMLInputElement).value).toBe('48.0');
  });

  it('commits an edit to an array-shaped vec as the whole patched array, via the field path (no dotted axis path)', () => {
    const onCommit = vi.fn();
    const binding = { component: 'bevy_ui::ui_node::ComputedNode', path: 'content_size', kind: 'vec2' as const };
    const { container } = render(<VecRow binding={binding} value={[360, 48]} onCommit={onCommit} />);
    const yInput = container.querySelectorAll<HTMLElement>('.num-cell')[1].querySelector('input') as HTMLInputElement;
    yInput.focus();
    yInput.value = '99';
    yInput.blur();
    expect(onCommit).toHaveBeenCalledWith('content_size', [360, 99]);
  });

  it('commits an array-shaped whole-value vec (binding.path === "") via the empty path', () => {
    const onCommit = vi.fn();
    const binding = { component: 'demo::Thing', path: '', kind: 'vec2' as const };
    const { container } = render(<VecRow binding={binding} value={[1, 2]} onCommit={onCommit} />);
    const xInput = container.querySelectorAll<HTMLElement>('.num-cell')[0].querySelector('input') as HTMLInputElement;
    xInput.focus();
    xInput.value = '7';
    xInput.blur();
    expect(onCommit).toHaveBeenCalledWith('', [7, 2]);
  });
});

describe('OpaqueJson', () => {
  it('renders collapsed by default with a compact one-line preview and no <pre>', () => {
    const value = { matrix: [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1], extra: 'x'.repeat(100) };
    const { container, getByTitle } = render(<OpaqueJson value={value} />);

    expect(container.querySelector('pre')).toBeNull();
    const preview = container.querySelector('.opaque-json-preview');
    expect(preview).toBeTruthy();
    expect(preview?.textContent?.length).toBeLessThanOrEqual(83); // 80 chars + '...'
    expect(preview?.textContent?.endsWith('...')).toBe(true);
    expect(getByTitle('Show raw value')).toBeTruthy();
  });

  it('reveals the raw <pre> value when the toggle is clicked, and collapses again on a second click', () => {
    const value = { a: 1, b: 2 };
    const { container, getByTitle } = render(<OpaqueJson value={value} />);

    fireEvent.click(getByTitle('Show raw value'));
    const pre = container.querySelector('pre');
    expect(pre).toBeTruthy();
    expect(pre?.textContent).toContain('"a": 1');
    expect(getByTitle('Hide raw value')).toBeTruthy();

    fireEvent.click(getByTitle('Hide raw value'));
    expect(container.querySelector('pre')).toBeNull();
  });
});
