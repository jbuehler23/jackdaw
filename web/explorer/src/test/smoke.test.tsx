import { render } from '@testing-library/preact';
import { describe, expect, it } from 'vitest';
import { App } from '../app';

describe('app shell', () => {
  it('renders', () => {
    const { container } = render(<App />);
    expect(container.textContent).toContain('Jackdaw Explorer');
  });
});
