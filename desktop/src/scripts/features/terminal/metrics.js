// Terminal panel: metrics sampling, sparkline SVG and the metrics panel.
function recordMetricsSample(id, metrics) {
  if (!metrics) return;
  if (!state.metricsHistory[id]) state.metricsHistory[id] = [];
  const list = state.metricsHistory[id];
  const prev = list[list.length - 1];
  const cpuDeltaMs = prev
    ? Math.max(0, (metrics.cpuUsageUsec - prev.cpuUsageUsec) / 1000)
    : metrics.cpuUsageUsec / 1000;
  list.push({
    ts: Date.now(),
    memBytes: metrics.memoryBytes || 0,
    cpuUsageUsec: metrics.cpuUsageUsec || 0,
    cpuDeltaMs,
  });
  if (list.length > 24) list.shift();
}

function buildSparklineSvg(values, strokeColor, fillColor) {
  const w = 220;
  const h = 50;
  if (!values.length) {
    return `<svg viewBox="0 0 ${w} ${h}" preserveAspectRatio="none" aria-hidden="true">
      <line x1="0" y1="${h - 6}" x2="${w}" y2="${h - 6}" stroke="var(--line)" stroke-dasharray="3 3" stroke-width="1" />
    </svg>`;
  }
  const maxVal = Math.max(...values, 1);
  const pts = values.map((v, i) => {
    const x = values.length === 1 ? w : (i / (values.length - 1)) * w;
    const y = h - 4 - (v / maxVal) * (h - 10);
    return [x.toFixed(1), y.toFixed(1)];
  });
  const line = pts.map(([x, y]) => `${x},${y}`).join(" ");
  const area = `0,${h} ${line} ${w},${h}`;
  return `<svg viewBox="0 0 ${w} ${h}" preserveAspectRatio="none" aria-hidden="true">
    <polygon points="${area}" fill="${fillColor}" />
    <polyline points="${line}" fill="none" stroke="${strokeColor}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round" />
  </svg>`;
}

function fmtUptime(secs) {
  const s = Number(secs) || 0;
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

function renderMetricsPanel(id, d) {
  const cardsEl = $("metrics-cards");
  const memWrap = $("spark-mem-svg");
  const cpuWrap = $("spark-cpu-svg");
  if (!cardsEl || !memWrap || !cpuWrap) return;

  const m = d.metrics;
  if (m) recordMetricsSample(id, m);
  const hist = state.metricsHistory[id] || [];

  const memText = m ? fmtBytes(m.memoryBytes) : "—";
  const memSub =
    m && m.memoryLimitBytes > 0
      ? t("drawer.metricLimit", { limit: fmtBytes(m.memoryLimitBytes) })
      : t("drawer.metricNoLimit");
  const cpuMs = m ? `${(m.cpuUsageUsec / 1000).toFixed(1)} ms` : "—";
  const pidsText = m ? String(m.pidsCurrent) : "—";
  const uptimeText = m ? fmtUptime(m.uptimeSecs) : "—";
  const srcText = m ? `PID ${m.pid} • ${m.source}` : t("drawer.metricNotRunning");

  cardsEl.innerHTML = `
    <div class="metric-card"><span class="mk">${esc(t("drawer.metricMem"))}</span><span class="mv">${esc(memText)}</span><span class="ms">${esc(memSub)}</span></div>
    <div class="metric-card"><span class="mk">${esc(t("drawer.metricCpu"))}</span><span class="mv">${esc(cpuMs)}</span><span class="ms">${esc(srcText)}</span></div>
    <div class="metric-card"><span class="mk">${esc(t("drawer.metricPids"))}</span><span class="mv">${esc(pidsText)}</span><span class="ms">${esc(t("drawer.metricRestartSub", { policy: d.restartPolicy || "no", count: d.restartCount || 0 }))}</span></div>
    <div class="metric-card"><span class="mk">${esc(t("drawer.metricUptime"))}</span><span class="mv">${esc(uptimeText)}</span><span class="ms">${esc(t("drawer.metricHealthSub", { health: tHealth(d.healthStatus || "none") }))}</span></div>`;

  $("spark-mem-val").textContent = memText;
  $("spark-cpu-val").textContent = cpuMs;
  memWrap.innerHTML = buildSparklineSvg(
    hist.map((p) => p.memBytes),
    "var(--accent-text)",
    "color-mix(in oklab, var(--accent) 18%, transparent)",
  );
  cpuWrap.innerHTML = buildSparklineSvg(
    hist.map((p) => p.cpuDeltaMs),
    "var(--moss)",
    "color-mix(in oklab, var(--moss) 18%, transparent)",
  );
}
