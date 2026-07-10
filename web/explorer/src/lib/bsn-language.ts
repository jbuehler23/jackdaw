// bsn-language.ts: CodeMirror StreamLanguage for BSN scene text, mirroring
// the PoC's tokenizeBsnLine rules (comments, strings, #labels, @templates,
// path::segment type paths incl. generics, bare identifiers, numbers).
import { HighlightStyle, StreamLanguage, type StringStream } from '@codemirror/language';
import { EditorView } from '@codemirror/view';
import { tags } from '@lezer/highlight';

// The stream parser is line-local; BSN has no multi-line tokens to track.
type BsnState = Record<string, never>;

const COMMENT = /^\/\/.*$/;
const STRING = /^"[^"]*"?/;
const LABEL = /^#[A-Za-z_][A-Za-z0-9_]*/;
const TYPE_PATH = /^@?[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_<>]*)+/;
const IDENT = /^[A-Za-z_][A-Za-z0-9_]*/;
const NUMBER = /^-?\d+\.?\d*/;

function match(stream: StringStream, re: RegExp): string | null {
  const rest = stream.string.slice(stream.pos);
  const m = rest.match(re);
  if (!m) return null;
  stream.pos += m[0].length;
  return m[0];
}

export const bsnLanguage = StreamLanguage.define<BsnState>({
  name: 'bsn',
  startState: () => ({}),
  token(stream) {
    if (match(stream, COMMENT)) return 'comment';
    if (match(stream, STRING)) return 'string';
    if (match(stream, LABEL)) return 'labelName';
    const path = match(stream, TYPE_PATH);
    if (path !== null) return path[0] === '@' ? 'macroName' : 'typeName';
    if (match(stream, IDENT)) return null;
    if (match(stream, NUMBER)) return 'number';
    stream.next();
    return null;
  },
});

export const bsnHighlight = HighlightStyle.define([
  { tag: tags.comment, color: '#77777D', fontStyle: 'italic' },
  { tag: tags.string, color: '#D9B373' },
  { tag: tags.labelName, color: '#FFCA39' },
  { tag: tags.macroName, color: '#B78CD1' },
  { tag: tags.typeName, color: '#8CA6D9' },
  { tag: tags.number, color: '#8CC78C' },
]);

export const bsnEditorTheme = EditorView.theme(
  {
    '&': {
      backgroundColor: '#232327',
      color: '#C8C8C8',
      fontSize: '12px',
      height: '100%',
    },
    '.cm-content': {
      fontFamily: 'var(--font-mono)',
      caretColor: '#C8C8C8',
      padding: '8px 12px',
    },
    '.cm-gutters': {
      backgroundColor: '#1F1F24',
      color: '#5B5B60',
      border: 'none',
      fontFamily: 'var(--font-mono)',
      fontSize: '11px',
    },
    '.cm-activeLine': { backgroundColor: 'transparent' },
    '.cm-activeLineGutter': { backgroundColor: 'transparent' },
    '&.cm-focused': { outline: 'none' },
    '.cm-scroller': { overflow: 'auto' },
  },
  { dark: true },
);
