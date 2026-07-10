import { render } from '@testing-library/preact';
import { fireEvent } from '@testing-library/preact';
import { describe, expect, it } from 'vitest';
import { OpaqueJson } from '../components/FieldEditors';

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
