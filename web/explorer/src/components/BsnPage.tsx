// BsnPage.tsx: BSN scene editor. Apply spawns the document into the running
// world via jackdaw/apply_bsn; Extract pulls the selected entity back out via
// jackdaw/entity_bsn. Ported from the PoC's editor toolbar/status/side-card
// layout, with CodeMirror replacing the PoC's textarea+pre overlay trick.
import { useEffect, useRef, useState } from 'preact/hooks';
import { EditorState, StateEffect, StateField } from '@codemirror/state';
import { EditorView, Decoration, type DecorationSet, keymap, lineNumbers } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { syntaxHighlighting } from '@codemirror/language';
import { Braces, Check, Link, RotateCw, Send, TriangleAlert } from 'lucide-preact';
import { Icon } from './Icon';
import { bsnEditorTheme, bsnHighlight, bsnLanguage } from '../lib/bsn-language';
import { jackdaw } from '../lib/brp';
import { capabilities } from '../lib/connection';
import { page, selectedEntity } from '../lib/state';
import { toast } from '../lib/toasts';
import { treePoll } from '../lib/treeData';

const DEFAULT_DOC = `// Applied to the running world via jackdaw/apply_bsn.
// Full type paths, exactly like on-disk BSN scenes.

#Root
bevy_transform::components::transform::Transform {
    translation: glam::Vec3 { x: 0.0, y: 1.0, z: 0.0 },
}
bevy_ecs::hierarchy::Children [
    #Light
    bevy_light::point_light::PointLight {
        intensity: 800.0,
        range: 14.0,
    }
    bevy_transform::components::transform::Transform {
        translation: glam::Vec3 { x: 0.0, y: 0.5, z: 0.0 },
    }
]
`;

const setErrorLine = StateEffect.define<number | null>();

const errorLineField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    let deco = value.map(tr.changes);
    for (const effect of tr.effects) {
      if (effect.is(setErrorLine)) {
        if (effect.value === null || effect.value >= tr.state.doc.lines) {
          deco = Decoration.none;
        } else {
          const line = tr.state.doc.line(effect.value + 1);
          deco = Decoration.set([Decoration.line({ attributes: { class: 'bsn-err-line' } }).range(line.from)]);
        }
      }
    }
    if (tr.docChanged) deco = Decoration.none;
    return deco;
  },
  provide: (field) => EditorView.decorations.from(field),
});

interface SpawnLogEntry {
  entity: number;
  label: string;
}

const spawnLog: SpawnLogEntry[] = [];

type Status = { kind: 'idle' | 'ok' | 'err'; message: string };

function parseErrorLine(message: string): number | null {
  const m = message.match(/line (\d+)/i);
  if (!m) return null;
  const line = Number.parseInt(m[1], 10);
  return Number.isFinite(line) ? line - 1 : null;
}

export function BsnPage() {
  const shellRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const [status, setStatus] = useState<Status>({
    kind: 'idle',
    message: 'Ready. Apply spawns this scene into the running world via jackdaw/apply_bsn.',
  });
  const [, forceRender] = useState(0);

  useEffect(() => {
    if (!shellRef.current) return;
    const view = new EditorView({
      state: EditorState.create({
        doc: DEFAULT_DOC,
        extensions: [
          lineNumbers(),
          history(),
          keymap.of([...defaultKeymap, ...historyKeymap]),
          bsnLanguage,
          syntaxHighlighting(bsnHighlight),
          errorLineField,
          bsnEditorTheme,
        ],
      }),
      parent: shellRef.current,
    });
    viewRef.current = view;
    return () => view.destroy();
  }, []);

  function docText(): string {
    return viewRef.current?.state.doc.toString() ?? '';
  }

  function replaceDoc(text: string) {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: text },
      effects: setErrorLine.of(null),
    });
  }

  function markErrorLine(line: number | null) {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({ effects: setErrorLine.of(line) });
    if (line !== null) {
      const doc = view.state.doc;
      if (line < doc.lines) {
        const pos = doc.line(line + 1).from;
        view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
      }
    }
  }

  async function applyDoc() {
    const source = docText();
    try {
      const { entities } = await jackdaw.applyBsn(source);
      markErrorLine(null);
      setStatus({
        kind: 'ok',
        message: `Applied. Spawned ${entities.length} ${entities.length === 1 ? 'entity' : 'entities'}: ${entities.join(', ')}.`,
      });
      for (const entity of entities) spawnLog.unshift({ entity, label: 'Entity' });
      forceRender((n) => n + 1);
      toast('ok', `jackdaw/apply_bsn: spawned ${entities.length} ${entities.length === 1 ? 'entity' : 'entities'}`);
      await treePoll.refresh();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      markErrorLine(parseErrorLine(message));
      setStatus({ kind: 'err', message });
      toast('err', `jackdaw/apply_bsn failed: ${message}`);
    }
  }

  function resetDoc() {
    replaceDoc(DEFAULT_DOC);
    setStatus({ kind: 'idle', message: 'Ready. Apply spawns this scene into the running world via jackdaw/apply_bsn.' });
  }

  async function extractDoc() {
    const entity = selectedEntity.value;
    if (entity === null) return;
    try {
      const { bsn } = await jackdaw.entityBsn(entity);
      replaceDoc(`// Extracted from entity ${entity} via jackdaw/entity_bsn.\n${bsn}`);
      setStatus({ kind: 'idle', message: `Extracted entity ${entity}. Edit and Apply to spawn a copy.` });
      toast('ok', `jackdaw/entity_bsn: extracted entity ${entity}`);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast('err', `jackdaw/entity_bsn failed: ${message}`);
    }
  }

  function jumpTo(entity: number) {
    selectedEntity.value = entity;
    page.value = 'entities';
  }

  const canApply = capabilities.value.has('jackdaw/apply_bsn');
  const canExtract = capabilities.value.has('jackdaw/entity_bsn');

  return (
    <div class="pane" style="flex:1">
      <div class="pane-header">
        <Icon of={Braces} />
        BSN
      </div>
      <div class="bsn-wrap">
        <div class="bsn-editor-col">
          <div class="bsn-toolbar">
            <span class="fname">scratch.bsn</span>
            <span class="spacer" />
            <button class="btn-ghost" onClick={resetDoc}>
              <Icon of={RotateCw} />
              Reset
            </button>
            {canApply ? (
              <button class="btn-primary" onClick={() => void applyDoc()}>
                <Icon of={Send} />
                Apply to world
              </button>
            ) : (
              <span style="font-size:11px;color:var(--text-secondary)">Apply needs jackdaw/apply_bsn</span>
            )}
          </div>
          <div class="editor-shell" ref={shellRef} />
          <div class={`bsn-status${status.kind === 'err' ? ' err' : status.kind === 'ok' ? ' ok' : ''}`}>
            <Icon of={status.kind === 'err' ? TriangleAlert : Check} />
            <span>{status.message}</span>
          </div>
        </div>
        <aside class="bsn-side">
          <div class="side-card">
            <h3>Live scene scripting</h3>
            <p>Write BSN and apply it to the running game without a rebuild. Parse and apply errors come back with a line number when the server reports one.</p>
          </div>
          <div class="side-card">
            <h3>Round-trip</h3>
            <p>Pull the selected entity (and its children) out of the world as BSN, edit it, and apply it back.</p>
            {canExtract ? (
              <button
                class="btn-ghost"
                style="width:100%;justify-content:center"
                disabled={selectedEntity.value === null}
                onClick={() => void extractDoc()}
              >
                <Icon of={Braces} />
                Extract from selected entity
              </button>
            ) : (
              <span style="font-size:11px;color:var(--text-secondary)">Extract needs jackdaw/entity_bsn</span>
            )}
          </div>
          <div class="side-card">
            <h3>Spawned this session</h3>
            <div class="spawn-log">
              {spawnLog.length === 0 ? (
                <p style="font-size:11px;color:var(--text-secondary)">Nothing yet.</p>
              ) : (
                spawnLog.map((entry) => (
                  <button class="entity-link" key={entry.entity} onClick={() => jumpTo(entry.entity)}>
                    <Icon of={Link} />
                    {entry.label} <span style="color:var(--text-secondary)">{entry.entity}</span>
                  </button>
                ))
              )}
            </div>
          </div>
        </aside>
      </div>
    </div>
  );
}
