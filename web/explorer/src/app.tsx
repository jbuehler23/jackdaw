// app.tsx: workbench shell. Topbar (brand, sim playback, connection, polling toggle),
// icon rail for page navigation, status bar, and toast overlay.
import { useEffect, useRef } from 'preact/hooks';
import {
  Activity,
  Boxes,
  Braces,
  ChartLine,
  Pause,
  Play,
  Search,
  SearchCode,
  Settings,
  SkipForward,
  Wifi,
  Box,
  Check,
  TriangleAlert,
  Zap,
} from 'lucide-preact';
import { Icon } from './components/Icon';
import { TreePanel } from './components/TreePanel';
import { Inspector } from './components/Inspector';
import { QueriesPage } from './components/QueriesPage';
import { StatsPage } from './components/StatsPage';
import { BsnPage } from './components/BsnPage';
import { CommandPalette, paletteOpen } from './components/CommandPalette';
import { page, pollingPaused, simPaused, type Page } from './lib/state';
import { appInfo, startConnection, status } from './lib/connection';
import { currentHost } from './lib/brp';
import { diagnosticsPoll } from './lib/stats';
import { fmtNumber } from './lib/format';
import { toasts, type ToastKind } from './lib/toasts';
import { stepSim, toggleSim, togglePolling } from './lib/commands';

const RAIL_PAGES: { id: Page; title: string; icon: typeof Boxes }[] = [
  { id: 'entities', title: 'Entities', icon: Boxes },
  { id: 'queries', title: 'Queries', icon: SearchCode },
  { id: 'stats', title: 'Stats', icon: ChartLine },
  { id: 'bsn', title: 'BSN', icon: Braces },
];

function connectionLabel(): string {
  if (pollingPaused.value) return 'Paused';
  if (status.value === 'connected') return 'Connected';
  if (status.value === 'error') return 'Error';
  return 'Connecting';
}

const TOAST_ICON: Record<ToastKind, typeof Check> = {
  ok: Check,
  err: TriangleAlert,
  info: Zap,
};

function Toasts() {
  return (
    <div class="toasts">
      {toasts.value.map((t) => (
        <div class={`toast ${t.kind}`} key={t.id}>
          <Icon of={TOAST_ICON[t.kind]} />
          <span>{t.message}</span>
        </div>
      ))}
    </div>
  );
}

// Drag-resizes the pane with the given id, mirroring the PoC's wireDivider:
// dirRight flips the drag direction for a divider that sits right of its pane.
function useDividerDrag(paneId: string, dirRight: boolean) {
  const drag = useRef<{ x: number; width: number } | null>(null);

  function onPointerDown(ev: PointerEvent) {
    const pane = document.getElementById(paneId);
    if (!pane) return;
    drag.current = { x: ev.clientX, width: pane.offsetWidth };
    (ev.currentTarget as HTMLElement).classList.add('dragging');
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
  }

  function onPointerMove(ev: PointerEvent) {
    const pane = document.getElementById(paneId);
    if (!drag.current || !pane) return;
    const dx = (ev.clientX - drag.current.x) * (dirRight ? -1 : 1);
    pane.style.width = `${Math.max(200, Math.min(560, drag.current.width + dx))}px`;
  }

  function onPointerUp(ev: PointerEvent) {
    drag.current = null;
    (ev.currentTarget as HTMLElement).classList.remove('dragging');
  }

  return { onPointerDown, onPointerMove, onPointerUp };
}

function EntitiesPage() {
  const divider = useDividerDrag('tree-pane', false);

  return (
    <>
      <TreePanel />
      <div
        class="divider"
        onPointerDown={divider.onPointerDown}
        onPointerMove={divider.onPointerMove}
        onPointerUp={divider.onPointerUp}
      />
      <Inspector />
    </>
  );
}

function PageContent() {
  switch (page.value) {
    case 'entities':
      return <EntitiesPage />;
    case 'queries':
      return <QueriesPage />;
    case 'stats':
      return <StatsPage />;
    case 'bsn':
      return <BsnPage />;
    default:
      return null;
  }
}

export function App() {
  useEffect(() => {
    void startConnection();
    diagnosticsPoll.start();
    return () => diagnosticsPoll.stop();
  }, []);

  useEffect(() => {
    function onKeyDown(ev: KeyboardEvent) {
      if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === 'k') {
        ev.preventDefault();
        paletteOpen.value = true;
      }
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, []);

  const diagnostics = diagnosticsPoll.data.value;

  return (
    <div class="app">
      <header class="topbar">
        <div class="brand">
          <Icon of={Box} />
          <span>
            <b>jackdaw</b> <i>explorer</i>
          </span>
        </div>
        <div class="playback">
          <span class="pb-label">sim</span>
          <button class="topbar-btn" style="height:22px" title="Game simulation (jackdaw/playback)" onClick={toggleSim}>
            <Icon of={simPaused.value ? Play : Pause} />
            <span>{simPaused.value ? 'Play' : 'Pause'}</span>
          </button>
          <button
            class="topbar-btn"
            style="height:22px"
            title="Step one frame"
            disabled={!simPaused.value}
            onClick={stepSim}
          >
            <Icon of={SkipForward} />
          </button>
        </div>
        <span class={`sim-paused-chip${simPaused.value ? ' on' : ''}`}>sim paused</span>
        <div class="conn">
          <span class="appname">
            app <b>{appInfo.value?.app_name ?? '…'}</b> · bevy <b>{appInfo.value?.bevy_version ?? '…'}</b>
          </span>
          <span class="host">{currentHost()}</span>
          <span class={`conn-dot${pollingPaused.value || status.value !== 'connected' ? ' paused' : ''}`} />
          <span>{connectionLabel()}</span>
        </div>
        <button
          class="topbar-btn"
          title="Pause the explorer's data polling (the game keeps running)"
          onClick={togglePolling}
        >
          <Icon of={pollingPaused.value ? Play : Pause} />
          <span>{pollingPaused.value ? 'Resume' : 'Polling'}</span>
        </button>
        <button
          class="topbar-btn"
          title="Search entities and run commands"
          onClick={() => {
            paletteOpen.value = true;
          }}
        >
          <Icon of={Search} />
          Search <span class="kbd">Ctrl K</span>
        </button>
      </header>

      <div class="main">
        <nav class="rail">
          {RAIL_PAGES.map((p) => (
            <button
              class={`rail-btn${page.value === p.id ? ' active' : ''}`}
              title={p.title}
              onClick={() => {
                page.value = p.id;
              }}
              key={p.id}
            >
              <Icon of={p.icon} />
            </button>
          ))}
          <div class="rail-spacer" />
          <button class="rail-btn" title="Settings" disabled>
            <Icon of={Settings} />
          </button>
        </nav>

        <div class="pages">
          <PageContent />
        </div>
      </div>

      <footer class="statusbar">
        <span class="seg">
          <Icon of={Wifi} />
          <span>{connectionLabel()}</span>
        </span>
        <div class="right">
          <span class="seg">
            <Icon of={Activity} />
            <b>{diagnostics?.fps != null ? fmtNumber(diagnostics.fps) : '…'}</b> fps
          </span>
          <span class="seg">
            <b>{diagnostics?.frame_time_ms != null ? fmtNumber(diagnostics.frame_time_ms) : '…'}</b> ms
          </span>
          <span class="seg">
            <Icon of={Boxes} />
            <b>{diagnostics?.entity_count != null ? String(diagnostics.entity_count) : '…'}</b> entities
          </span>
        </div>
      </footer>

      <Toasts />
      <CommandPalette />
    </div>
  );
}
