// StatsPage.tsx: fps/frame-time/entity-count tiles fed by the shell's shared
// diagnostics poll, each backed by a 90-sample RingBuffer for its sparkline.
import { useEffect, useState } from 'preact/hooks';
import { Activity, Zap } from 'lucide-preact';
import { Icon } from './Icon';
import { Sparkline } from './Sparkline';
import { diagnosticsPoll } from '../lib/stats';
import { capabilities } from '../lib/connection';
import { RingBuffer } from '../lib/ring';
import { fmtNumber } from '../lib/format';

const fpsHistory = new RingBuffer(90);
const frameHistory = new RingBuffer(90);
const entityHistory = new RingBuffer(90);

export function StatsPage() {
  const [, forceRender] = useState(0);
  const diagnostics = diagnosticsPoll.data.value;
  const hasDiagnostics = capabilities.value.has('jackdaw/diagnostics');

  useEffect(() => {
    if (!diagnostics) return;
    fpsHistory.push(diagnostics.fps ?? 0);
    frameHistory.push(diagnostics.frame_time_ms ?? 0);
    entityHistory.push(diagnostics.entity_count);
    forceRender((n) => n + 1);
  }, [diagnostics]);

  return (
    <div class="pane" style="flex:1">
      <div class="pane-header">
        <Icon of={Activity} />
        Diagnostics
      </div>
      {!hasDiagnostics ? (
        <div class="stats-wrap">
          <div class="stats-note">
            <Icon of={Zap} />
            <span>
              Served by the <code>jackdaw/diagnostics</code> method from <code>JackdawRemotePlugin</code>. Apps
              running plain BRP still get the tree, inspector, and queries; this page shows an upgrade hint instead.
            </span>
          </div>
        </div>
      ) : (
        <div class="stats-wrap">
          <div class="stats-grid">
            <div class="stat-tile">
              <div class="st-label">Frames per second</div>
              <div class="st-value">{diagnostics?.fps != null ? fmtNumber(diagnostics.fps) : '…'}</div>
              <Sparkline data={fpsHistory.values()} color="#60A5FA" />
            </div>
            <div class="stat-tile">
              <div class="st-label">Frame time</div>
              <div class="st-value">
                {diagnostics?.frame_time_ms != null ? fmtNumber(diagnostics.frame_time_ms) : '…'}
                <small>ms</small>
              </div>
              <Sparkline data={frameHistory.values()} color="#FFCA39" />
            </div>
            <div class="stat-tile">
              <div class="st-label">Entities</div>
              <div class="st-value">{diagnostics?.entity_count ?? '…'}</div>
              <Sparkline data={entityHistory.values()} color="#42B983" />
            </div>
          </div>
          <div class="stats-note">
            <Icon of={Zap} />
            <span>
              Served by the <code>jackdaw/diagnostics</code> method from <code>JackdawRemotePlugin</code>.
            </span>
          </div>
        </div>
      )}
    </div>
  );
}
