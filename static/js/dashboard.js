const REFRESH_INTERVAL_MS = 15000;
const UPTIME_TICK_MS = 1000;
const CHART_METRIC_KEYS = [
  'process.cpu.usage_percent',
  'process.memory.resident_bytes',
  'process.io.read_bytes_per_sec',
  'process.io.write_bytes_per_sec',
];
const PINNED_METRIC_KEYS = [
  'process.cpu.usage_percent',
  'process.memory.resident_bytes',
  'process.io.read_bytes_per_sec',
  'process.io.write_bytes_per_sec',
];
const METRIC_LABEL_OVERRIDES = {
  'process.cpu.usage_percent': 'CPU-Auslastung (Prozess)',
  'process.memory.resident_bytes': 'Arbeitsspeicher RSS (Prozess)',
  'process.io.read_bytes_per_sec': 'I/O Lesen pro Sekunde',
  'process.io.write_bytes_per_sec': 'I/O Schreiben pro Sekunde',
};
const HISTORY_RANGE_DEFAULT = '1h';
const HISTORY_RANGES = ['1h', '3h', '24h'];
const HISTORY_RETENTION_MS = 24 * 60 * 60 * 1000;
const AUDIT_HISTORY_RANGE_DEFAULT = '1h';
const AUDIT_HISTORY_RANGES = ['1h', '3h', '24h'];
const AUDIT_HISTORY_LIMIT = 1000;

const metaEl = document.querySelector('[data-meta]');
const alertEl = document.querySelector('[data-alert]');
const svcBody = document.querySelector('[data-services]');
const metaServices = document.querySelector('[data-service-meta]');
const appName = document.querySelector('[data-app-name]');
const appVersion = document.querySelector('[data-app-version]');
const uptimeEl = document.querySelector('[data-uptime]');
const healthValue = document.querySelector('[data-health]');
const healthNote = document.querySelector('[data-health-note]');
const tokenInput = document.querySelector('[data-token-input]');
const tokenStatus = document.querySelector('[data-token-status]');
const testButton = document.querySelector('[data-test-btn]');
const bulkStartBtn = document.querySelector('[data-bulk-start]');
const bulkStopBtn = document.querySelector('[data-bulk-stop]');
const bulkRestartBtn = document.querySelector('[data-bulk-restart]');
const auditBody = document.querySelector('[data-audit-events]');
const auditMeta = document.querySelector('[data-audit-meta]');
const auditRefreshBtn = document.querySelector('[data-audit-refresh]');
const telemetryNote = document.querySelector('[data-telemetry-note]');
const metricChartCanvas = document.querySelector('[data-metric-chart]');
const metricPlaceholder = document.querySelector('[data-chart-empty]');
const metricLegend = document.querySelector('[data-metric-legend]');
const seriesToggleContainer = document.querySelector('[data-series-toggle]');
const metricListBody = document.querySelector('[data-metric-list]');
const serviceMetricsBody = document.querySelector('[data-service-metrics]');
const serviceMetricsMeta = document.querySelector('[data-service-metrics-meta]');
const serviceSortSelect = document.querySelector('[data-service-sort]');
const metricCpuValue = document.querySelector('[data-metric-cpu]');
const metricCpuNote = document.querySelector('[data-metric-cpu-note]');
const metricMemValue = document.querySelector('[data-metric-mem]');
const metricMemNote = document.querySelector('[data-metric-mem-note]');
const metricIoValue = document.querySelector('[data-metric-io]');
const metricIoNote = document.querySelector('[data-metric-io-note]');
const chartTooltip = document.querySelector('[data-chart-tooltip]');
const chartTooltipTime = document.querySelector('[data-tooltip-time]');
const chartTooltipBody = document.querySelector('[data-tooltip-body]');
const pageButtons = Array.from(document.querySelectorAll('[data-page-trigger]'));
const pageContainers = new Map(
  Array.from(document.querySelectorAll('[data-page]')).map((el) => [el.dataset.page, el]),
);
const historyRangeButtons = Array.from(document.querySelectorAll('[data-history-range]'));
const auditRangeButtons = Array.from(document.querySelectorAll('[data-audit-range]'));

const TOKEN_KEY = 'fenrir-control-plane-token';
const PAGE_STORAGE_KEY = 'fenrir-control-plane-page';

let refreshHandle = null;
let uptimeHandle = null;
let uptimeBaseSeconds = null;
let uptimeAnchor = null;
let isLoading = false;
let auditCache = [];
let servicesCache = new Map();
let eventSource = null;
let sseWarned = false;
const metricHistory = [];
const METRIC_HISTORY_LIMIT = 20000;
const METRIC_HISTORY_CHART_LIMIT = 2000;
const METRIC_SERIES_MAX = 4;
const METRIC_COLORS = ['#4f8efd', '#33d5c4', '#f4d35e', '#ed6a5a', '#c792ea', '#6ad1ff'];
const METRIC_AXIS_OVERRIDES = {
  'process.cpu.usage_percent': 'percent',
};
const AXIS_DEFAULT = 'primary';
const AXIS_LABEL_OVERRIDES = {
  primary: '',
  percent: 'CPU %',
};
const SELECTABLE_SERIES = [
  { key: 'process.cpu.usage_percent', label: 'CPU', default: false },
  { key: 'process.memory.resident_bytes', label: 'RAM', default: true },
  { key: 'process.io.read_bytes_per_sec', label: 'I/O Lesen', default: false },
  { key: 'process.io.write_bytes_per_sec', label: 'I/O Schreiben', default: false },
];
const selectableSeriesMap = new Map(SELECTABLE_SERIES.map((entry) => [entry.key, entry]));
const activeSeries = new Set(
  SELECTABLE_SERIES.filter((entry) => entry.default !== false).map((entry) => entry.key),
);
const numberFormatter = new Intl.NumberFormat('de-DE');
let metricCtx = null;
const SERVICE_STATUS_KEYS = ['starting', 'active', 'degraded', 'failed', 'standby', 'stopped'];
const SERVICE_STATUS_FALLBACK = 'services.status.other';
const SERVICE_TAG_KEYS = ['core', 'platform', 'auxiliary'];
let chartHoverState = null;
let currentHistoryRange = HISTORY_RANGE_DEFAULT;
let historyLoaded = false;
let historyLoading = false;
let currentAuditRange = AUDIT_HISTORY_RANGE_DEFAULT;
let auditHistoryLoaded = false;
let auditHistoryLoading = false;
const SERVICE_RESOURCE_STALE_MS = 60_000;
const SERVICE_SORT_LABELS = {
  'cpu-desc': 'CPU ↓',
  'cpu-asc': 'CPU ↑',
  'memory-desc': 'RAM ↓',
  'memory-asc': 'RAM ↑',
  'updated-desc': 'Aktualisiert ↓',
  'updated-asc': 'Aktualisiert ↑',
  'id-asc': 'Service A→Z',
  'id-desc': 'Service Z→A',
};
const SERVICE_SORT_DEFAULT = 'cpu-desc';
if (serviceSortSelect && !serviceSortSelect.value) {
  serviceSortSelect.value = SERVICE_SORT_DEFAULT;
}
let serviceMetricsSort = parseServiceSort(
  serviceSortSelect ? serviceSortSelect.value : SERVICE_SORT_DEFAULT,
);
let serviceResourceData = [];

const loadToken = () => {
  try {
    return localStorage.getItem(TOKEN_KEY) ?? '';
  } catch (_) {
    return '';
  }
};

const saveToken = (value) => {
  try {
    if (!value) {
      localStorage.removeItem(TOKEN_KEY);
    } else {
      localStorage.setItem(TOKEN_KEY, value);
    }
  } catch (_) {}
};

const rangeToMillis = (range) => {
  switch (range) {
    case '3h':
      return 3 * 60 * 60 * 1000;
    case '24h':
    case '1d':
      return 24 * 60 * 60 * 1000;
    case '1h':
    default:
      return 60 * 60 * 1000;
  }
};

const currentToken = () => tokenInput.value.trim();

const setHistoryButtonsActive = (range) => {
  historyRangeButtons.forEach((button) => {
    const active = button.dataset.historyRange === range;
    button.dataset.active = active ? 'true' : 'false';
  });
};

const setAuditButtonsActive = (range) => {
  auditRangeButtons.forEach((button) => {
    const active = button.dataset.auditRange === range;
    button.dataset.active = active ? 'true' : 'false';
  });
};

const pruneMetricHistory = () => {
  const cutoff = Date.now() - HISTORY_RETENTION_MS;
  while (metricHistory.length > 0 && metricHistory[0].timestamp < cutoff) {
    metricHistory.shift();
  }
  if (metricHistory.length > METRIC_HISTORY_LIMIT) {
    metricHistory.splice(0, metricHistory.length - METRIC_HISTORY_LIMIT);
  }
};

const getHistoryForChart = () => {
  if (metricHistory.length <= METRIC_HISTORY_CHART_LIMIT) {
    return metricHistory;
  }
  const step = Math.ceil(metricHistory.length / METRIC_HISTORY_CHART_LIMIT);
  const sampled = [];
  for (let idx = 0; idx < metricHistory.length; idx += step) {
    sampled.push(metricHistory[idx]);
  }
  const last = metricHistory[metricHistory.length - 1];
  if (sampled[sampled.length - 1] !== last) {
    sampled.push(last);
  }
  return sampled;
};

const applyHistorySamples = (samples) => {
  metricHistory.length = 0;
  if (Array.isArray(samples) && samples.length > 0) {
    samples
      .slice()
      .sort((a, b) => a.timestamp_ms - b.timestamp_ms)
      .forEach((sample) => {
        metricHistory.push({
          timestamp: sample.timestamp_ms,
          counters: sample.metrics ?? {},
        });
      });
    pruneMetricHistory();
    if (metricHistory.length > 0) {
      const latestCounters = metricHistory[metricHistory.length - 1].counters;
      renderMetricList(latestCounters);
      updateTelemetrySummary(latestCounters);
    }
  }
  renderMetricChart();
};

const fetchTelemetryHistory = async (range, { background = false } = {}) => {
  if (historyLoading) {
    return;
  }
  historyLoading = true;
  try {
    const response = await fetchWithToken(`/metrics/history?range=${encodeURIComponent(range)}`);
    if (!response.ok) {
      if (!background && telemetryNote) {
        telemetryNote.textContent = `Historie nicht verfügbar (${response.status}).`;
      }
      return;
    }
    const body = await response.json();
    applyHistorySamples(body.samples ?? []);
    if (telemetryNote) {
      const stamp = new Date().toLocaleTimeString('de-DE');
      telemetryNote.textContent = `Stand: ${stamp} · Range ${body.range ?? range}`;
    }
    historyLoaded = true;
  } catch (error) {
    if (!background && telemetryNote) {
      telemetryNote.textContent = 'Telemetrie-Historie nicht verfügbar.';
    }
  } finally {
    historyLoading = false;
  }
};

const trimAuditCache = () => {
  if (!Array.isArray(auditCache)) {
    auditCache = [];
    return;
  }
  const rangeMs = rangeToMillis(currentAuditRange);
  const cutoff = Date.now() - rangeMs;
  auditCache = auditCache.filter((entry) => {
    const ts = Date.parse(entry.timestamp ?? '');
    return !Number.isFinite(ts) || ts >= cutoff;
  });
  if (auditCache.length > AUDIT_HISTORY_LIMIT) {
    auditCache.length = AUDIT_HISTORY_LIMIT;
  }
};

const applyAuditEvents = (events) => {
  if (!Array.isArray(events)) {
    auditCache = [];
  } else {
    auditCache = events;
  }
  trimAuditCache();
  renderAuditCache();
};

const fetchAuditHistory = async (range, { background = false } = {}) => {
  if (auditHistoryLoading) {
    return;
  }
  auditHistoryLoading = true;
  try {
    const response = await fetchWithToken(`/audit/history?range=${encodeURIComponent(range)}&limit=${AUDIT_HISTORY_LIMIT}`);
    if (!response.ok) {
      if (!background) {
        showAlert(`Audit-Historie nicht verfügbar (${response.status}).`);
      }
      return;
    }
    const body = await response.json();
    applyAuditEvents(body.events ?? []);
    auditMeta.textContent = `${auditCache.length} Einträge · Range ${body.range ?? range}`;
    auditHistoryLoaded = true;
  } catch (error) {
    if (!background) {
      showAlert('Audit-Historie nicht verfügbar.');
    }
  } finally {
    auditHistoryLoading = false;
  }
};

const authHeaders = () => {
  const token = currentToken();
  if (!token) {
    return {};
  }
  return {
    Authorization: token.startsWith('Bearer ') ? token : `Bearer ${token}`,
  };
};

const setTokenUi = (token) => {
  if (!token) {
    tokenStatus.textContent = 'Token nicht gesetzt – Anfragen erfolgen ohne Auth.';
    testButton.disabled = true;
  } else {
    tokenStatus.textContent = 'Token aktiv – geschützte Endpunkte verwenden nun Autorisierung.';
    testButton.disabled = false;
  }
};

const showAlert = (message) => {
  if (!message) {
    alertEl.dataset.visible = 'false';
    alertEl.textContent = '';
    return;
  }
  alertEl.dataset.visible = 'true';
  alertEl.textContent = message;
};

const formatDuration = (seconds) => {
  if (seconds == null) return '–';
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) {
    return `${days}d ${hours}h`;
  }
  if (hours > 0) {
    return `${hours}h ${minutes}m`;
  }
  return `${minutes}m`;
};

const formatTimestamp = (value) => {
  if (!value) return '–';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString('de-DE', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  });
};

const escapeHtml = (value) => {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
};

const formatNumber = (value) => {
  if (!Number.isFinite(value)) {
    return '–';
  }
  return numberFormatter.format(value);
};

const withAlpha = (hex, alpha) => {
  if (typeof hex !== 'string') {
    return hex;
  }
  const normalized = hex.startsWith('#') ? hex.slice(1) : hex;
  if (normalized.length !== 6) {
    return hex;
  }
  const r = parseInt(normalized.slice(0, 2), 16);
  const g = parseInt(normalized.slice(2, 4), 16);
  const b = parseInt(normalized.slice(4, 6), 16);
  const clampedAlpha = Math.min(Math.max(alpha ?? 1, 0), 1);
  if (!Number.isFinite(r) || !Number.isFinite(g) || !Number.isFinite(b)) {
    return hex;
  }
  return `rgba(${r}, ${g}, ${b}, ${clampedAlpha})`;
};

const formatChartTime = (timestamp) => {
  if (!Number.isFinite(timestamp)) {
    return '';
  }
  const date = new Date(timestamp);
  return date.toLocaleTimeString('de-DE', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
};

const formatMetricLabel = (key) => {
  if (!key) {
    return 'metric';
  }
  return String(key)
    .replace(/^(services|process)\./, '')
    .replace(/\./g, ' › ')
    .replace(/_/g, ' ');
};

const axisForMetric = (key) => {
  if (!key) {
    return AXIS_DEFAULT;
  }
  if (Object.prototype.hasOwnProperty.call(METRIC_AXIS_OVERRIDES, key)) {
    return METRIC_AXIS_OVERRIDES[key];
  }
  if (key.endsWith('.percent') || key.includes('.percent')) {
    return 'percent';
  }
  if (key.includes('.cpu.')) {
    return 'percent';
  }
  return AXIS_DEFAULT;
};

const axisLabelFor = (axisKey, key) => {
  if (Object.prototype.hasOwnProperty.call(AXIS_LABEL_OVERRIDES, axisKey)) {
    const label = AXIS_LABEL_OVERRIDES[axisKey];
    if (label) {
      return label;
    }
  }
  if (!key) {
    return '';
  }
  if (axisKey === 'percent') {
    return 'CPU %';
  }
  if (key.includes('.memory.') || key.includes('.bytes')) {
    return 'Bytes';
  }
  if (key.includes('.io.') && key.includes('_per_sec')) {
    return 'Bytes/s';
  }
  return '';
};

const isSeriesSelectable = (key) => selectableSeriesMap.has(key);

const isSeriesActive = (key) => {
  if (!key) {
    return false;
  }
  if (!isSeriesSelectable(key)) {
    return true;
  }
  return activeSeries.has(key);
};

const sanitizeSeriesId = (key) => {
  return `series-toggle-${key.replace(/[^a-zA-Z0-9_-]/g, '-')}`;
};

const renderSeriesToggles = () => {
  if (!seriesToggleContainer || SELECTABLE_SERIES.length === 0) {
    return;
  }
  seriesToggleContainer.innerHTML = SELECTABLE_SERIES.map((entry) => {
    const id = sanitizeSeriesId(entry.key);
    const checked = activeSeries.has(entry.key) ? 'checked' : '';
    return `
      <label class="series-toggle" for="${id}">
        <input type="checkbox" id="${id}" value="${entry.key}" ${checked} data-series-checkbox="${entry.key}">
        <span>${escapeHtml(entry.label)}</span>
      </label>
    `;
  }).join('');

  const checkboxes = seriesToggleContainer.querySelectorAll('[data-series-checkbox]');
  checkboxes.forEach((checkbox) => {
    checkbox.addEventListener('change', (event) => {
      const target = event.target;
      if (!(target instanceof HTMLInputElement)) {
        return;
      }
      const key = target.value;
      if (!key) {
        return;
      }
      if (target.checked) {
        activeSeries.add(key);
      } else {
        activeSeries.delete(key);
      }
      renderMetricChart();
    });
  });
};

const formatPercent = (value, fractionDigits = 0) => {
  if (!Number.isFinite(value)) {
    return '–';
  }
  return `${value.toFixed(fractionDigits)} %`;
};

const formatBytes = (value) => {
  if (!Number.isFinite(value)) {
    return '–';
  }
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
  let amount = value;
  let unitIndex = 0;
  while (amount >= 1024 && unitIndex < units.length - 1) {
    amount /= 1024;
    unitIndex += 1;
  }
  const precision = amount >= 10 || unitIndex === 0 ? 0 : 1;
  return `${amount.toFixed(precision)} ${units[unitIndex]}`;
};

const formatThroughput = (value) => {
  if (!Number.isFinite(value)) {
    return '0 B/s';
  }
  if (value === 0) {
    return '0 B/s';
  }
  return `${formatBytes(value)}/s`;
};

function parseServiceSort(value) {
  const [rawKey, rawDir] = String(value || SERVICE_SORT_DEFAULT).split('-');
  const key = ['cpu', 'memory', 'updated', 'id', 'peak'].includes(rawKey)
    ? rawKey
    : 'cpu';
  const direction = rawDir === 'asc' ? 'asc' : 'desc';
  return { key, direction };
}

function serviceSortValue(sort) {
  return `${sort.key}-${sort.direction}`;
}

function formatServiceSortLabel(sort) {
  const key = serviceSortValue(sort);
  return SERVICE_SORT_LABELS[key] || key;
}

function normalizeServiceResourceEntry(entry) {
  const cpu =
    typeof entry?.cpu_percent === 'number' && Number.isFinite(entry.cpu_percent)
      ? entry.cpu_percent
      : null;
  const memory =
    typeof entry?.memory_bytes === 'number' && Number.isFinite(entry.memory_bytes)
      ? entry.memory_bytes
      : null;
  const peak =
    typeof entry?.memory_peak_bytes === 'number' && Number.isFinite(entry.memory_peak_bytes)
      ? entry.memory_peak_bytes
      : null;
  const updatedAtMs = entry?.updated_at ? Date.parse(entry.updated_at) : NaN;
  return {
    id: entry?.id || 'unbekannt',
    cpu,
    memory,
    peak,
    updatedAtMs: Number.isFinite(updatedAtMs) ? updatedAtMs : null,
    reported: Boolean(entry?.reported),
    stale: Boolean(entry?.stale),
  };
}

function compareServiceResource(a, b, sort) {
  const direction = sort.direction === 'asc' ? 1 : -1;
  if (sort.key === 'id') {
    return a.id.localeCompare(b.id) * direction;
  }
  const va = getServiceSortValue(a, sort.key);
  const vb = getServiceSortValue(b, sort.key);
  if (va == null && vb == null) {
    return a.id.localeCompare(b.id);
  }
  if (va == null) {
    return 1;
  }
  if (vb == null) {
    return -1;
  }
  if (va < vb) {
    return -1 * direction;
  }
  if (va > vb) {
    return 1 * direction;
  }
  return a.id.localeCompare(b.id);
}

function getServiceSortValue(item, key) {
  switch (key) {
    case 'cpu':
      return item.cpu;
    case 'memory':
      return item.memory;
    case 'peak':
      return item.peak ?? item.memory;
    case 'updated':
      return item.updatedAtMs;
    default:
      return item.cpu;
  }
}

function formatRelativeTime(timestampMs) {
  if (!Number.isFinite(timestampMs)) {
    return '–';
  }
  const diffSeconds = Math.round((Date.now() - timestampMs) / 1000);
  if (diffSeconds < 0) {
    return 'sofort';
  }
  if (diffSeconds < 10) {
    return 'jetzt';
  }
  if (diffSeconds < 60) {
    return `vor ${diffSeconds}s`;
  }
  const diffMinutes = Math.round(diffSeconds / 60);
  if (diffMinutes < 60) {
    return `vor ${diffMinutes}m`;
  }
  const diffHours = Math.round(diffMinutes / 60);
  if (diffHours < 24) {
    return `vor ${diffHours}h`;
  }
  const diffDays = Math.round(diffHours / 24);
  if (diffDays < 7) {
    return `vor ${diffDays}d`;
  }
  return new Date(timestampMs).toLocaleString('de-DE', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  });
}

function renderServiceMetrics(emptyMessage) {
  if (!serviceMetricsBody) {
    return;
  }
  if (serviceSortSelect) {
    const desired = serviceSortValue(serviceMetricsSort);
    if (serviceSortSelect.value !== desired) {
      serviceSortSelect.value = desired;
    }
  }

  if (serviceResourceData.length === 0) {
    const message = emptyMessage || 'Keine Ressourcen-Metriken gemeldet.';
    serviceMetricsBody.innerHTML = `<tr><td colspan="5">${escapeHtml(message)}</td></tr>`;
    if (serviceMetricsMeta) {
      serviceMetricsMeta.textContent = '0 Services · Keine Daten';
    }
    return;
  }

  const sorted = [...serviceResourceData].sort((a, b) =>
    compareServiceResource(a, b, serviceMetricsSort),
  );
  serviceMetricsBody.innerHTML = sorted
    .map((entry) => {
      const cpuText =
        entry.cpu != null
          ? escapeHtml(formatPercent(entry.cpu, entry.cpu < 10 ? 1 : 0))
          : entry.reported
          ? '<span class="metric-nodata">keine Daten</span>'
          : '–';
      const memoryText =
        entry.memory != null
          ? escapeHtml(formatBytes(entry.memory))
          : entry.reported
          ? '<span class="metric-nodata">keine Daten</span>'
          : '–';
      const peakText =
        entry.peak != null
          ? escapeHtml(formatBytes(entry.peak))
          : entry.memory != null
          ? escapeHtml(formatBytes(entry.memory))
          : entry.reported
          ? '<span class="metric-nodata">keine Daten</span>'
          : '–';
      const updatedText = (() => {
        if (!entry.reported) {
          return '<span class="metric-nodata">nicht gemeldet</span>';
        }
        if (entry.updatedAtMs == null) {
          return '<span class="metric-nodata">keine Aktualisierung</span>';
        }
        const relative = escapeHtml(formatRelativeTime(entry.updatedAtMs));
        const stale = entry.stale || Date.now() - entry.updatedAtMs >= SERVICE_RESOURCE_STALE_MS;
        return stale
          ? `${relative} <span class="metric-stale">veraltet</span>`
          : relative;
      })();
      return `
        <tr>
          <td>${escapeHtml(entry.id)}</td>
          <td class="numeric">${cpuText}</td>
          <td class="numeric">${memoryText}</td>
          <td class="numeric">${peakText}</td>
          <td>${updatedText}</td>
        </tr>
      `;
    })
    .join('');

  if (serviceMetricsMeta) {
    const label = formatServiceSortLabel(serviceMetricsSort);
    const prefix = `${sorted.length} Services`;
    serviceMetricsMeta.textContent = label
      ? `${prefix} · Sortierung ${label}`
      : prefix;
  }
}

function resetServiceMetrics(message) {
  serviceResourceData = [];
  renderServiceMetrics(message);
}

function setServiceMetricsData(resources) {
  if (!Array.isArray(resources)) {
    resetServiceMetrics();
    return;
  }
  serviceResourceData = resources.map(normalizeServiceResourceEntry);
  renderServiceMetrics();
}

const formatTooltipValue = (key, value) => {
  if (!Number.isFinite(value)) {
    return '–';
  }
  if (key.includes('.cpu.') || key.endsWith('.percent')) {
    return formatPercent(value, value < 10 ? 1 : 0);
  }
  if (key.includes('.memory.') || key.includes('.bytes_total')) {
    return formatBytes(value);
  }
  if (key.includes('.io.') && key.includes('_per_sec')) {
    return formatThroughput(value);
  }
  return formatNumber(value);
};

const hideChartTooltip = () => {
  if (!chartTooltip) {
    return;
  }
  chartTooltip.dataset.visible = 'false';
  chartTooltip.style.transform = 'translate(-9999px, -9999px)';
};

const filterChartCounters = (counters) => {
  if (!counters) {
    return {};
  }
  const preferred = CHART_METRIC_KEYS.filter((key) => key in counters);
  if (preferred.length === 0) {
    return {};
  }
  const mapped = {};
  preferred.forEach((key) => {
    mapped[key] = counters[key];
  });
  return mapped;
};

const resetTelemetrySummary = () => {
  if (metricCpuValue) {
    metricCpuValue.textContent = '–';
  }
  if (metricCpuNote) {
    metricCpuNote.textContent = 'Keine Prozessdaten verfügbar.';
  }
  if (metricMemValue) {
    metricMemValue.textContent = '–';
  }
  if (metricMemNote) {
    metricMemNote.textContent = 'Keine Prozessdaten verfügbar.';
  }
  if (metricIoValue) {
    metricIoValue.textContent = '–';
  }
  if (metricIoNote) {
    metricIoNote.textContent = 'Keine Prozessdaten verfügbar.';
  }
};

const updateTelemetrySummary = (counters) => {
  if (!counters || !Object.keys(counters).some((key) => key.startsWith('process.'))) {
    return;
  }

  if (metricCpuValue) {
    const cpuPercent = Number(counters['process.cpu.usage_percent']);
    if (Number.isFinite(cpuPercent)) {
      metricCpuValue.textContent = formatPercent(cpuPercent, cpuPercent < 10 ? 1 : 0);
      if (metricCpuNote) {
        if (cpuPercent >= 90) {
          metricCpuNote.textContent = 'Warnung: sehr hohe Auslastung.';
        } else if (cpuPercent >= 70) {
          metricCpuNote.textContent = 'Hinweis: erhöhte CPU-Last.';
        } else {
          metricCpuNote.textContent = 'CPU-Last unkritisch.';
        }
      }
    }
  }

  if (metricMemValue) {
    const residentBytes = Number(counters['process.memory.resident_bytes']);
    const virtualBytes = Number(counters['process.memory.virtual_bytes']);
    if (Number.isFinite(residentBytes)) {
      metricMemValue.textContent = formatBytes(residentBytes);
      if (metricMemNote) {
        if (Number.isFinite(virtualBytes) && virtualBytes > 0) {
          metricMemNote.textContent = `Virtuell: ${formatBytes(virtualBytes)}`;
        } else {
          metricMemNote.textContent = 'Residenter Speicher (RSS).';
        }
      }
    }
  }

  if (metricIoValue) {
    const readRate = Number(counters['process.io.read_bytes_per_sec']);
    const writeRate = Number(counters['process.io.write_bytes_per_sec']);
    const combinedRate = [readRate, writeRate]
      .filter((value) => Number.isFinite(value))
      .reduce((sum, value) => sum + value, 0);
    if (Number.isFinite(combinedRate)) {
      metricIoValue.textContent = formatThroughput(combinedRate);
      if (metricIoNote) {
        const readText = Number.isFinite(readRate) ? formatThroughput(readRate) : '0 B/s';
        const writeText = Number.isFinite(writeRate) ? formatThroughput(writeRate) : '0 B/s';
        metricIoNote.textContent = `Lesen ${readText} · Schreiben ${writeText}`;
      }
    }
  }
};

const renderMetricLegend = (entries) => {
  if (!metricLegend) {
    return;
  }
  const items = Array.isArray(entries) ? entries : [];
  if (items.length === 0) {
    metricLegend.innerHTML = '<span class="legend-empty">Es liegen noch keine Messwerte vor.</span>';
    return;
  }
  metricLegend.innerHTML = items
    .map((entry) => {
      const baseLabel = entry.label ?? METRIC_LABEL_OVERRIDES[entry.key] ?? formatMetricLabel(entry.key);
      const label = escapeHtml(baseLabel);
      const valueText = escapeHtml(formatTooltipValue(entry.key, entry.value));
      const color = entry.color || '#4f8efd';
      const axisLabel = entry.axisLabel ? escapeHtml(entry.axisLabel) : '';
      const axisHtml = axisLabel
        ? `<span class="legend-axis">${axisLabel}</span>`
        : '';
      return `
        <span class="legend-item">
          <span class="legend-swatch" style="background:${color}"></span>
          <span class="legend-label">${label}</span>
          ${axisHtml}
          <span class="legend-value">${valueText}</span>
        </span>
      `;
    })
    .join('');
};

const setChartEmpty = (empty, message) => {
  if (!metricChartCanvas || !metricPlaceholder) {
    return;
  }
  if (empty) {
    metricChartCanvas.style.display = 'none';
    metricPlaceholder.dataset.visible = 'true';
    if (message) {
      metricPlaceholder.textContent = message;
    }
    renderMetricLegend([]);
    hideChartTooltip();
  } else {
    metricChartCanvas.style.display = 'block';
    metricPlaceholder.dataset.visible = 'false';
    if (message) {
      metricPlaceholder.textContent = message;
    }
  }
};

const renderMetricList = (counters) => {
  if (!metricListBody) {
    return;
  }
  const entries = Object.entries(counters || {}).filter(([, value]) => Number.isFinite(value));
  if (entries.length === 0) {
    metricListBody.innerHTML = '<tr><td colspan="2">Keine Telemetriedaten gemeldet.</td></tr>';
    return;
  }

  const pinned = PINNED_METRIC_KEYS
    .map((key) => [key, Number(counters?.[key])])
    .filter(([, value]) => Number.isFinite(value));
  const seen = new Set(pinned.map(([key]) => key));

  const dynamic = entries
    .filter(([key]) => !seen.has(key))
    .sort((a, b) => b[1] - a[1]);

  const combined = [...pinned, ...dynamic].slice(0, 20);
  metricListBody.innerHTML = combined
    .map(([name, value]) => {
      const label = escapeHtml(METRIC_LABEL_OVERRIDES[name] ?? formatMetricLabel(name));
      const valueText = escapeHtml(formatTooltipValue(name, value));
      return `<tr><td>${label}</td><td>${valueText}</td></tr>`;
    })
    .join('');
};

const renderMetricChart = () => {
  if (!metricChartCanvas) {
    return;
  }
  if (!metricCtx) {
    metricCtx = metricChartCanvas.getContext('2d');
  }
  if (!metricCtx) {
    return;
  }
  chartHoverState = null;
  const history = getHistoryForChart();
  if (history.length === 0) {
    metricCtx.clearRect(0, 0, metricChartCanvas.width, metricChartCanvas.height);
    renderMetricLegend([]);
    const placeholderText = metricPlaceholder ? metricPlaceholder.textContent : '';
    setChartEmpty(true, placeholderText || 'Keine Telemetriedaten gemeldet.');
    return;
  }

  const keys = new Set();
  history.forEach((sample) => {
    Object.keys(sample.counters).forEach((key) => keys.add(key));
  });
  if (keys.size === 0) {
    renderMetricLegend([]);
    setChartEmpty(true, 'Keine Telemetriedaten gemeldet.');
    return;
  }

  const latestCounters = metricHistory[metricHistory.length - 1]?.counters ?? {};
  const presentKeys = Array.from(keys);
  const activeKeySet = new Set(presentKeys.filter((key) => isSeriesActive(key)));

  if (activeKeySet.size === 0) {
    const message = presentKeys.some((key) => isSeriesSelectable(key))
      ? 'Keine Metriken ausgewählt.'
      : 'Keine Telemetriedaten gemeldet.';
    renderMetricLegend([]);
    hideChartTooltip();
    setChartEmpty(true, message);
    return;
  }

  setChartEmpty(false);
  const ctx = metricCtx;
  const width = metricChartCanvas.width;
  const height = metricChartCanvas.height;
  ctx.clearRect(0, 0, width, height);

  const padding = 48;
  const plotWidth = width - padding * 2;
  const plotHeight = height - padding * 2;

  const latest = metricHistory[metricHistory.length - 1]?.counters ?? {};
  const prioritizedSeries = [];
  CHART_METRIC_KEYS.forEach((key) => {
    if (activeKeySet.has(key) && !prioritizedSeries.includes(key)) {
      prioritizedSeries.push(key);
    }
  });
  const otherSeries = Array.from(activeKeySet).filter(
    (key) => !prioritizedSeries.includes(key),
  );
  otherSeries.sort((a, b) => (latestCounters[b] ?? 0) - (latestCounters[a] ?? 0));
  const series = [...prioritizedSeries, ...otherSeries].slice(0, METRIC_SERIES_MAX);

  if (series.length === 0) {
    renderMetricLegend([]);
    setChartEmpty(true, 'Keine Telemetriedaten gemeldet.');
    return;
  }

  const seriesAxes = new Map();
  const axisStats = new Map();

  const ensureAxisStats = (axisKey, key) => {
    if (!axisStats.has(axisKey)) {
      axisStats.set(axisKey, {
        min: Infinity,
        max: -Infinity,
        representative: key,
        keys: [key],
        label: axisLabelFor(axisKey, key),
      });
    } else {
      const stats = axisStats.get(axisKey);
      if (stats) {
        if (!stats.keys.includes(key)) {
          stats.keys.push(key);
        }
        if (!stats.representative) {
          stats.representative = key;
          stats.label = axisLabelFor(axisKey, key);
        }
      }
    }
  };

  series.forEach((key) => {
    const axisKey = axisForMetric(key);
    seriesAxes.set(key, axisKey);
    ensureAxisStats(axisKey, key);
  });

  history.forEach((sample) => {
    series.forEach((key) => {
      const axisKey = seriesAxes.get(key) || AXIS_DEFAULT;
      const stats = axisStats.get(axisKey);
      if (!stats) {
        return;
      }
      const value = sample.counters[key];
      if (!Number.isFinite(value)) {
        return;
      }
      if (value < stats.min) stats.min = value;
      if (value > stats.max) stats.max = value;
    });
  });

  axisStats.forEach((stats, axisKey) => {
    if (!Number.isFinite(stats.min) || !Number.isFinite(stats.max)) {
      stats.min = 0;
      stats.max = axisKey === 'percent' ? 1 : 1;
    }
    if (axisKey === 'percent') {
      stats.min = 0;
      const observed = Math.max(stats.max, 0);
      stats.max = observed > 0 ? observed : 1;
    } else if (stats.min === stats.max) {
      if (stats.min === 0) {
        stats.max = 1;
      } else if (stats.min > 0) {
        stats.min = 0;
      } else {
        stats.max = stats.min + 1;
      }
    }
    stats.label = axisLabelFor(axisKey, stats.representative);
  });

  if (axisStats.size === 0) {
    renderMetricLegend([]);
    setChartEmpty(true, 'Keine Telemetriedaten gemeldet.');
    return;
  }

  const resolvePrimaryAxisKey = () => {
    if (axisStats.has(AXIS_DEFAULT)) {
      return AXIS_DEFAULT;
    }
    const iterator = axisStats.keys().next();
    return iterator && !iterator.done ? iterator.value : null;
  };

  const primaryAxisKey = resolvePrimaryAxisKey();
  if (!primaryAxisKey) {
    renderMetricLegend([]);
    setChartEmpty(true, 'Keine Telemetriedaten gemeldet.');
    return;
  }
  const primaryAxis = axisStats.get(primaryAxisKey);
  const secondaryAxisKey = (() => {
    for (const key of axisStats.keys()) {
      if (key !== primaryAxisKey) {
        return key;
      }
    }
    return null;
  })();
  const secondaryAxis = secondaryAxisKey ? axisStats.get(secondaryAxisKey) : null;

  const toX = (index) => {
    const ratio = index / Math.max(history.length - 1, 1);
    return padding + ratio * plotWidth;
  };
  const toY = (value, axisKey = primaryAxisKey) => {
    const stats = axisStats.get(axisKey) || primaryAxis;
    if (!stats) {
      return height - padding;
    }
    const min = stats.min;
    const max = stats.max;
    if (!Number.isFinite(min) || !Number.isFinite(max) || max === min) {
      return height - padding;
    }
    const clamped = Math.min(Math.max(value, min), max);
    const ratio = (clamped - min) / (max - min);
    return height - padding - ratio * plotHeight;
  };

  const firstTimestamp = history[0]?.timestamp;
  const lastTimestamp = history[history.length - 1]?.timestamp;

  const legendEntries = series.map((key, index) => {
    const axisKey = seriesAxes.get(key) || primaryAxisKey;
    const stats = axisStats.get(axisKey);
    return {
      key,
      axis: axisKey,
      axisLabel: stats?.label ?? AXIS_LABEL_OVERRIDES[axisKey] ?? '',
      value: latestCounters[key] ?? 0,
      color: METRIC_COLORS[index % METRIC_COLORS.length],
    };
  });
  renderMetricLegend(legendEntries);

  ctx.save();
  const background = ctx.createLinearGradient(0, padding, 0, height - padding);
  background.addColorStop(0, 'rgba(62, 124, 214, 0.20)');
  background.addColorStop(1, 'rgba(8, 16, 30, 0.82)');
  ctx.fillStyle = background;
  ctx.fillRect(padding, padding, plotWidth, plotHeight);
  ctx.restore();

  ctx.save();
  ctx.lineWidth = 1;
  ctx.strokeStyle = 'rgba(120, 160, 220, 0.18)';
  ctx.setLineDash([4, 8]);
  const gridY = 4;
  for (let i = 1; i < gridY; i += 1) {
    const y = padding + (plotHeight / gridY) * i;
    ctx.beginPath();
    ctx.moveTo(padding, y);
    ctx.lineTo(padding + plotWidth, y);
    ctx.stroke();
  }
  const gridX = Math.min(Math.max(history.length - 1, 1), 6);
  for (let i = 1; i < gridX; i += 1) {
    const x = padding + (plotWidth / gridX) * i;
    ctx.beginPath();
    ctx.moveTo(x, padding);
    ctx.lineTo(x, padding + plotHeight);
    ctx.stroke();
  }
  ctx.restore();

  ctx.save();
  ctx.strokeStyle = 'rgba(140, 180, 240, 0.45)';
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.moveTo(padding, padding);
  ctx.lineTo(padding, height - padding);
  ctx.lineTo(width - padding, height - padding);
  ctx.stroke();
  ctx.restore();

  const hoverSeries = [];
  const timestamps = history.map((sample) => sample.timestamp ?? NaN);
  const stepX = plotWidth / Math.max(history.length - 1, 1);
  series.forEach((key, index) => {
    const color = METRIC_COLORS[index % METRIC_COLORS.length];
    const axisKey = seriesAxes.get(key) || primaryAxisKey;
    const values = new Array(history.length).fill(null);
    const points = new Array(history.length).fill(null);
    const pathPoints = [];

    history.forEach((sample, idx) => {
      const value = sample.counters[key];
      if (!Number.isFinite(value)) {
        return;
      }
      const x = padding + stepX * idx;
      const y = toY(value, axisKey);
      const point = { x, y };
      points[idx] = point;
      pathPoints.push(point);
      values[idx] = value;
    });

    if (pathPoints.length === 0) {
      return;
    }

    if (pathPoints.length > 1) {
      ctx.save();
      ctx.fillStyle = withAlpha(color, 0.18);
      ctx.beginPath();
      ctx.moveTo(pathPoints[0].x, height - padding);
      pathPoints.forEach((point) => ctx.lineTo(point.x, point.y));
      ctx.lineTo(pathPoints[pathPoints.length - 1].x, height - padding);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
    }

    ctx.save();
    ctx.strokeStyle = color;
    ctx.lineWidth = 2.2;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';
    ctx.beginPath();
    let started = false;
    points.forEach((point) => {
      if (!point) {
        return;
      }
      if (!started) {
        ctx.moveTo(point.x, point.y);
        started = true;
      } else {
        ctx.lineTo(point.x, point.y);
      }
    });
    ctx.stroke();
    ctx.restore();

    const lastPoint = pathPoints[pathPoints.length - 1];
    if (lastPoint) {
      ctx.save();
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(lastPoint.x, lastPoint.y, 4, 0, Math.PI * 2);
      ctx.fill();
      ctx.restore();
    }

    hoverSeries.push({
      key,
      label: formatMetricLabel(key),
      color,
      values,
      points,
      axis: axisKey,
    });
  });

  const formatAxisValue = (axisKey, value) => {
    if (!Number.isFinite(value)) {
      return '';
    }
    const stats = axisStats.get(axisKey);
    const representative = stats?.representative || series.find((candidate) => {
      return (seriesAxes.get(candidate) || primaryAxisKey) === axisKey;
    });
    const key = representative || series[0];
    return formatTooltipValue(key, value);
  };

  const axisTitleFor = (axisKey) => {
    const stats = axisStats.get(axisKey);
    return stats?.label || AXIS_LABEL_OVERRIDES[axisKey] || '';
  };

  ctx.save();
  ctx.fillStyle = 'rgba(156, 190, 255, 0.7)';
  ctx.font = '12px "Inter", system-ui, sans-serif';
  ctx.textAlign = 'left';

  const primaryMaxLabel = formatAxisValue(primaryAxisKey, primaryAxis?.max ?? NaN);
  if (primaryMaxLabel) {
    ctx.textBaseline = 'top';
    ctx.fillText(primaryMaxLabel, padding + 4, padding + 4);
  }
  const primaryMinLabel = formatAxisValue(primaryAxisKey, primaryAxis?.min ?? NaN);
  if (primaryMinLabel) {
    ctx.textBaseline = 'bottom';
    ctx.fillText(primaryMinLabel, padding + 4, height - padding - 4);
  }

  const primaryTitle = axisTitleFor(primaryAxisKey);
  if (primaryTitle) {
    ctx.save();
    ctx.font = '10px "Inter", system-ui, sans-serif';
    ctx.textBaseline = 'bottom';
    ctx.fillText(primaryTitle, padding + 4, padding - 6);
    ctx.restore();
  }

  if (secondaryAxis && secondaryAxisKey) {
    ctx.textAlign = 'right';
    const secondaryMaxLabel = formatAxisValue(secondaryAxisKey, secondaryAxis.max);
    if (secondaryMaxLabel) {
      ctx.textBaseline = 'top';
      ctx.fillText(secondaryMaxLabel, width - padding - 4, padding + 4);
    }
    const secondaryMinLabel = formatAxisValue(secondaryAxisKey, secondaryAxis.min);
    if (secondaryMinLabel) {
      ctx.textBaseline = 'bottom';
      ctx.fillText(secondaryMinLabel, width - padding - 4, height - padding - 4);
    }
    const secondaryTitle = axisTitleFor(secondaryAxisKey);
    if (secondaryTitle) {
      ctx.save();
      ctx.font = '10px "Inter", system-ui, sans-serif';
      ctx.textBaseline = 'bottom';
      ctx.fillText(secondaryTitle, width - padding - 4, padding - 6);
      ctx.restore();
    }
  }

  ctx.textBaseline = 'top';
  ctx.textAlign = 'left';
  if (Number.isFinite(firstTimestamp)) {
    ctx.fillText(formatChartTime(firstTimestamp), padding, height - padding + 8);
  }
  if (Number.isFinite(lastTimestamp)) {
    ctx.textAlign = 'right';
    ctx.fillText(formatChartTime(lastTimestamp), width - padding, height - padding + 8);
  }
  ctx.restore();

  if (hoverSeries.length > 0) {
    chartHoverState = {
      padding,
      plotWidth,
      width,
      height,
      series: hoverSeries,
      timestamps,
    };
  } else {
    chartHoverState = null;
  }
};

const recordMetricSample = (counters) => {
  const numericEntries = Object.entries(counters || {}).filter(([, value]) => {
    return typeof value === 'number' && Number.isFinite(value);
  });
  if (numericEntries.length === 0) {
    metricHistory.length = 0;
    renderMetricList({});
    renderMetricChart();
    if (telemetryNote) {
      telemetryNote.textContent = 'Keine Telemetriedaten verfügbar.';
    }
    resetTelemetrySummary();
    setChartEmpty(true, 'Keine Telemetriedaten gemeldet.');
    return;
  }

  const snapshot = Object.fromEntries(numericEntries);
  const chartSnapshot = filterChartCounters(snapshot);
  if (Object.keys(chartSnapshot).length > 0) {
    const now = Date.now();
    const last = metricHistory[metricHistory.length - 1];
    if (last && Math.abs(now - last.timestamp) < 500) {
      last.timestamp = now;
      last.counters = chartSnapshot;
    } else {
      metricHistory.push({ timestamp: now, counters: chartSnapshot });
    }
    pruneMetricHistory();
  } else {
    hideChartTooltip();
  }
  renderMetricList(snapshot);
  updateTelemetrySummary(snapshot);
  renderMetricChart();
  if (telemetryNote) {
    telemetryNote.textContent = `Stand: ${new Date().toLocaleTimeString('de-DE')} · Range ${currentHistoryRange}`;
  }
};

const markTelemetryUnavailable = (message) => {
  metricHistory.length = 0;
  if (metricCtx && metricChartCanvas) {
    metricCtx.clearRect(0, 0, metricChartCanvas.width, metricChartCanvas.height);
  }
  renderMetricList({});
  renderMetricLegend([]);
  resetTelemetrySummary();
  hideChartTooltip();
  setChartEmpty(true, message || 'Telemetrie nicht verfügbar.');
  if (telemetryNote) {
    telemetryNote.textContent = message || 'Telemetrie nicht verfügbar.';
  }
  historyLoaded = false;
  resetServiceMetrics(message || 'Telemetrie nicht verfügbar.');
};

const handleChartHover = (event) => {
  if (!metricChartCanvas || !chartHoverState || !chartHoverState.timestamps || chartHoverState.timestamps.length === 0) {
    hideChartTooltip();
    return;
  }
  const rect = metricChartCanvas.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0) {
    hideChartTooltip();
    return;
  }
  const scaleX = metricChartCanvas.width / rect.width;
  let canvasX = (event.clientX - rect.left) * scaleX;
  const { padding, plotWidth } = chartHoverState;
  const maxX = padding + plotWidth;
  canvasX = Math.min(Math.max(canvasX, padding), maxX);

  const total = chartHoverState.timestamps.length;
  if (total === 0) {
    hideChartTooltip();
    return;
  }
  const ratio = total === 1 ? 0 : (canvasX - padding) / plotWidth;
  let index = Math.round(ratio * (total - 1));
  index = Math.min(Math.max(index, 0), total - 1);

  renderMetricChart();
  const state = chartHoverState;
  if (!state || state.series.length === 0) {
    hideChartTooltip();
    return;
  }

  const hoverX = total === 1 ? padding : padding + (index / (total - 1)) * state.plotWidth;
  const ctx = metricCtx;
  ctx.save();
  ctx.strokeStyle = 'rgba(236, 246, 255, 0.34)';
  ctx.lineWidth = 1.5;
  ctx.setLineDash([3, 4]);
  ctx.beginPath();
  ctx.moveTo(hoverX, state.padding);
  ctx.lineTo(hoverX, state.height - state.padding);
  ctx.stroke();
  ctx.restore();

  const rows = [];
  state.series.forEach((series) => {
    const value = series.values[index];
    if (!Number.isFinite(value)) {
      return;
    }
    const point = series.points[index];
    if (point) {
      ctx.save();
      ctx.fillStyle = series.color;
      ctx.strokeStyle = 'rgba(4, 16, 32, 0.82)';
      ctx.lineWidth = 1.2;
      ctx.beginPath();
      ctx.arc(point.x, point.y, 4.5, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
      ctx.restore();
    }
    rows.push({
      key: series.key,
      label: series.label,
      value,
      color: series.color,
    });
  });

  if (!chartTooltip || !chartTooltipBody || !chartTooltipTime || rows.length === 0) {
    hideChartTooltip();
    return;
  }

  rows.sort((a, b) => b.value - a.value);
  chartTooltipBody.innerHTML = rows
    .map((row) => {
      const formattedValue = formatTooltipValue(row.key, row.value);
      return `
        <div class="tooltip-row">
          <span class="tooltip-swatch" style="background:${row.color}"></span>
          <span class="tooltip-label">${escapeHtml(row.label)}</span>
          <span class="tooltip-value">${formattedValue}</span>
        </div>
      `;
    })
    .join('');

  const timestamp = state.timestamps[index];
  chartTooltipTime.textContent = Number.isFinite(timestamp)
    ? formatChartTime(timestamp)
    : '–';

  chartTooltip.dataset.visible = 'true';
  const container = metricChartCanvas.parentElement;
  if (!container) {
    return;
  }
  const containerRect = container.getBoundingClientRect();
  const tooltipRect = chartTooltip.getBoundingClientRect();
  const offsetX = event.clientX - containerRect.left + 12;
  const offsetY = event.clientY - containerRect.top + 12;
  const maxLeft = containerRect.width - tooltipRect.width - 12;
  const maxTop = containerRect.height - tooltipRect.height - 12;
  const finalX = Math.max(12, Math.min(offsetX, maxLeft));
  const finalY = Math.max(12, Math.min(offsetY, maxTop));
  chartTooltip.style.transform = `translate(${finalX}px, ${finalY}px)`;
};

const collectServiceMetrics = () => {
  const counters = {
    'services.total': servicesCache.size,
    'services.critical': 0,
  };
  SERVICE_STATUS_KEYS.forEach((key) => {
    counters[`services.status.${key}`] = 0;
  });
  counters[SERVICE_STATUS_FALLBACK] = 0;
  SERVICE_TAG_KEYS.forEach((key) => {
    counters[`services.tag.${key}`] = 0;
  });
  servicesCache.forEach((svc) => {
    const statusKey = `services.status.${svc.status ?? 'other'}`;
    if (statusKey in counters) {
      counters[statusKey] += 1;
    } else {
      counters[SERVICE_STATUS_FALLBACK] += 1;
    }
    if (Array.isArray(svc.tags)) {
      svc.tags.forEach((tag) => {
        const tagKey = `services.tag.${tag}`;
        if (tagKey in counters) {
          counters[tagKey] += 1;
        }
      });
    }
    if (svc.critical) {
      counters['services.critical'] += 1;
    }
  });
  return counters;
};

const pushServiceMetricsSample = () => {
  if (!servicesCache || servicesCache.size === 0) {
    return;
  }
  renderMetricList(collectServiceMetrics());
};

const getStoredPage = () => {
  try {
    const value = localStorage.getItem(PAGE_STORAGE_KEY);
    if (value && pageContainers.has(value)) {
      return value;
    }
  } catch (_) {}
  return 'overview';
};

const setActivePage = (name, { persist = true } = {}) => {
  const target = pageContainers.has(name) ? name : 'overview';
  pageContainers.forEach((element, key) => {
    element.dataset.visible = key === target ? 'true' : 'false';
  });
  pageButtons.forEach((button) => {
    const active = button.dataset.pageTrigger === target;
    button.dataset.active = active ? 'true' : 'false';
    button.setAttribute('aria-selected', active ? 'true' : 'false');
  });
  if (persist) {
    try {
      localStorage.setItem(PAGE_STORAGE_KEY, target);
    } catch (_) {}
  }
  if (target === 'telemetry') {
    window.requestAnimationFrame(() => renderMetricChart());
    if (!historyLoaded && !historyLoading) {
      fetchTelemetryHistory(currentHistoryRange, { background: true }).catch(() => {});
    }
  } else {
    hideChartTooltip();
  }
};

resetTelemetrySummary();
setChartEmpty(true, 'Noch keine Telemetriedaten verfügbar.');

const initialPage = getStoredPage();
setActivePage(initialPage, { persist: false });

pageButtons.forEach((button) => {
  button.addEventListener('click', () => {
    const target = button.dataset.pageTrigger || 'overview';
    setActivePage(target);
  });
});

setHistoryButtonsActive(currentHistoryRange);
historyRangeButtons.forEach((button) => {
  button.addEventListener('click', () => {
    const targetRange = button.dataset.historyRange;
    if (!targetRange || targetRange === currentHistoryRange || historyLoading) {
      return;
    }
    currentHistoryRange = targetRange;
    setHistoryButtonsActive(targetRange);
    historyLoaded = false;
    fetchTelemetryHistory(targetRange).catch(() => {});
  });
});

if (serviceSortSelect) {
  serviceSortSelect.addEventListener('change', () => {
    serviceMetricsSort = parseServiceSort(serviceSortSelect.value);
    renderServiceMetrics();
  });
}

setAuditButtonsActive(currentAuditRange);
auditRangeButtons.forEach((button) => {
  button.addEventListener('click', () => {
    const targetRange = button.dataset.auditRange;
    if (!targetRange || targetRange === currentAuditRange || auditHistoryLoading) {
      return;
    }
    currentAuditRange = targetRange;
    setAuditButtonsActive(targetRange);
    auditHistoryLoaded = false;
    fetchAuditHistory(targetRange).catch(() => {});
  });
});

if (metricChartCanvas) {
  metricChartCanvas.addEventListener('mousemove', handleChartHover);
  metricChartCanvas.addEventListener('mouseleave', () => {
    hideChartTooltip();
    if (chartHoverState) {
      window.requestAnimationFrame(() => renderMetricChart());
    }
  });
}

const renderActor = (actor) => {
  if (!actor || actor.kind === 'system') {
    return '<span class="metadata-chip">system</span>';
  }
  const chips = [];
  if (actor.role) {
    chips.push(`<span class="metadata-chip">role=${actor.role}</span>`);
  }
  if (actor.user_id) {
    chips.push(`<span class="metadata-chip">user=${actor.user_id}</span>`);
  } else if (actor.user_id_redacted) {
    chips.push('<span class="metadata-chip" data-redacted="true">user=&lt;redacted&gt;</span>');
  }
  return chips.join('');
};

const renderMetadata = (metadata) => {
  if (!metadata || metadata.length === 0) {
    return '<span class="metadata-chip" data-redacted="false">none</span>';
  }
  return metadata
    .map((entry) => {
      const safeValue = entry.value || '–';
      return `<span class="metadata-chip" data-redacted="${entry.redacted}">${entry.key}=${safeValue}</span>`;
    })
    .join('');
};

const renderAuditCache = () => {
  if (!auditCache || auditCache.length === 0) {
    auditBody.innerHTML = '<tr><td colspan="6">Keine Audit-Ereignisse vorhanden.</td></tr>';
    auditMeta.textContent = `0 Einträge · Range ${currentAuditRange}`;
    return;
  }
  auditMeta.textContent = `${auditCache.length} Einträge · Range ${currentAuditRange}`;
  auditBody.innerHTML = auditCache
    .map((event) => {
      const outcomeClass = event.outcome === 'success'
        ? 'pill success'
        : event.outcome === 'denied'
        ? 'pill denied'
        : 'pill failure';
      return `
        <tr>
          <td>${formatTimestamp(event.timestamp)}</td>
          <td>${event.action}</td>
          <td>${event.target}</td>
          <td><span class="${outcomeClass}">${event.outcome}</span></td>
          <td><div class="metadata-chips">${renderActor(event.actor)}</div></td>
          <td><div class="metadata-chips">${renderMetadata(event.metadata)}</div></td>
        </tr>
      `;
    })
    .join('');
};

const renderAuditEvents = (payload) => {
  auditCache = payload?.events ?? [];
  renderAuditCache();
};

const handleAuditPush = (event) => {
  if (!event) {
    return;
  }
  const eventTime = Date.parse(event.timestamp ?? '');
  const rangeMs = rangeToMillis(currentAuditRange);
  const cutoff = Date.now() - rangeMs;
  if (Number.isFinite(eventTime) && eventTime < cutoff) {
    return;
  }

  const key = `${event.timestamp}|${event.action}|${event.target}`;
  const existingIndex = auditCache.findIndex((entry) => {
    return `${entry.timestamp}|${entry.action}|${entry.target}` === key;
  });
  if (existingIndex !== -1) {
    auditCache.splice(existingIndex, 1);
  }
  auditCache.unshift(event);
  trimAuditCache();
  renderAuditCache();
};

const connectEventStream = () => {
  if (!window.EventSource) {
    return;
  }
  if (eventSource) {
    eventSource.close();
    eventSource = null;
  }
  const token = currentToken();
  const url = token
    ? `/events/stream?token=${encodeURIComponent(token)}`
    : '/events/stream';
  try {
    eventSource = new EventSource(url);
  } catch (error) {
    if (!sseWarned) {
      showAlert('Event-Stream konnte nicht aufgebaut werden.');
      sseWarned = true;
    }
    return;
  }
  sseWarned = false;
  eventSource.addEventListener('audit', (event) => {
    try {
      const payload = JSON.parse(event.data);
      handleAuditPush(payload);
    } catch (error) {
      console.warn('Fehler beim Verarbeiten von Audit-SSE', error);
    }
  });
  eventSource.addEventListener('service-state', (event) => {
    try {
      const payload = JSON.parse(event.data);
      applyServiceState(payload);
    } catch (error) {
      console.warn('Fehler beim Verarbeiten von Service-SSE', error);
    }
  });
  eventSource.addEventListener('audit-error', (event) => {
    if (!sseWarned) {
      showAlert(event.data || 'Event-Stream verweigert. Bitte Token prüfen.');
      sseWarned = true;
    }
  });
  eventSource.addEventListener('error', () => {
    if (!sseWarned) {
      showAlert('Event-Stream unterbrochen – Fallback auf Polling.');
      sseWarned = true;
    }
  });
};

const updateUptimeTicker = () => {
  if (uptimeBaseSeconds == null || uptimeAnchor == null) {
    uptimeEl.textContent = '–';
    return;
  }
  const elapsedSeconds = Math.max(0, Math.floor((Date.now() - uptimeAnchor) / 1000));
  uptimeEl.textContent = formatDuration(uptimeBaseSeconds + elapsedSeconds);
};

const setUptimeBase = (seconds) => {
  if (seconds == null) {
    uptimeBaseSeconds = null;
    uptimeAnchor = null;
    uptimeEl.textContent = '–';
    if (uptimeHandle !== null) {
      window.clearInterval(uptimeHandle);
      uptimeHandle = null;
    }
    return;
  }
  uptimeBaseSeconds = seconds;
  uptimeAnchor = Date.now();
  updateUptimeTicker();
  if (uptimeHandle === null) {
    uptimeHandle = window.setInterval(updateUptimeTicker, UPTIME_TICK_MS);
  }
};

const statusClass = (status) => {
  switch (status) {
    case 'active':
      return 'status-pill status-active';
    case 'starting':
      return 'status-pill status-starting';
    case 'degraded':
      return 'status-pill status-degraded';
    case 'failed':
      return 'status-pill status-failed';
    case 'stopped':
      return 'status-pill status-stopped';
    default:
      return 'status-pill status-starting';
  }
};

const renderTags = (tags) => {
  if (!tags || tags.length === 0) {
    return '<span class="tag">none</span>';
  }
  return tags
    .map((tag) => {
      let cls = 'tag';
      if (tag === 'core') cls += ' tag-core';
      if (tag === 'platform') cls += ' tag-platform';
      return `<span class="${cls}">${tag}</span>`;
    })
    .join('');
};

const renderRowActions = (svc) => {
  const disabled = svc.tags.includes('core');
  const disableAttr = disabled ? 'disabled' : '';
  return `
    <div class="row-actions">
      <button type="button" data-service-action="start" data-service-id="${svc.id}">Start</button>
      <button type="button" data-service-action="stop" data-service-id="${svc.id}" ${disableAttr}>Stop</button>
      <button type="button" data-service-action="restart" data-service-id="${svc.id}" ${disableAttr}>Restart</button>
    </div>
  `;
};

const updateMeta = (info) => {
  appName.textContent = info.app?.name ?? 'Fenrir';
  appVersion.textContent = `Version ${info.app?.version ?? 'unbekannt'}`;
  const host = info.http?.host ?? 'localhost';
  const port = info.http?.port ?? 'n/a';
  metaEl.innerHTML = `
    <span>${host}:${port}</span>
    <span>Control Plane</span>
  `;
};

const renderServicesTable = () => {
  if (servicesCache.size === 0) {
    svcBody.innerHTML = '<tr><td colspan="6">Keine Services registriert.</td></tr>';
    metaServices.textContent = '0 Services';
    return;
  }
  const items = Array.from(servicesCache.values()).sort((a, b) =>
    (a.id || '').localeCompare(b.id || ''),
  );
  metaServices.textContent = `${items.length} Services`;
  svcBody.innerHTML = items
    .map((svc) => {
      const status = svc.status ?? 'unknown';
      const note = svc.note ?? '–';
      const tags = renderTags(svc.tags ?? []);
      return `
        <tr>
          <td class="service-id">${svc.id}</td>
          <td class="service-name">${svc.name ?? svc.id}</td>
          <td class="status-cell"><span class="${statusClass(status)}" title="${status}">${status}</span></td>
          <td class="tags-cell"><div class="tag-list">${tags}</div></td>
          <td class="note-cell">${note}</td>
          <td>${renderRowActions(svc)}</td>
        </tr>
      `;
    })
    .join('');
};

const updateServices = (payload) => {
  servicesCache = new Map();
  if (payload?.services && Array.isArray(payload.services)) {
    payload.services.forEach((svc) => {
      servicesCache.set(svc.id, {
        ...svc,
        note: svc.note ?? '–',
        tags: Array.isArray(svc.tags) ? svc.tags : [],
      });
    });
  }
  renderServicesTable();
  pushServiceMetricsSample();
};

const applyServiceState = (event) => {
  if (!event || !event.id) {
    return;
  }
  const existing = servicesCache.get(event.id) || {};
  const tags = Array.isArray(event.tags) ? event.tags : existing.tags ?? [];
  const updated = {
    ...existing,
    id: event.id,
    name: event.name ?? existing.name ?? event.id,
    kind: event.kind ?? existing.kind ?? 'other',
    status: event.status ?? existing.status ?? 'unknown',
    note: event.note ?? existing.note ?? '–',
    tags,
    critical: event.critical ?? existing.critical ?? false,
  };
  servicesCache.set(event.id, updated);
  renderServicesTable();
  pushServiceMetricsSample();
};

const updateHealth = (live, ready) => {
  if (live && ready) {
    healthValue.textContent = 'bereit';
    healthNote.textContent = 'System lebt & ist einsatzbereit.';
  } else if (live && !ready) {
    healthValue.textContent = 'initialisiert';
    healthNote.textContent = 'Liveness ok, Ready-Checks ausstehend.';
  } else {
    healthValue.textContent = 'nicht erreichbar';
    healthNote.textContent = 'Bitte Logs prüfen oder Admin informieren.';
  }
};

const fetchWithToken = (url, options = {}) => {
  const headers = { ...options.headers, ...authHeaders() };
  return fetch(url, { ...options, headers });
};

const performServiceAction = async (id, action, force = false) => {
  const url = `/services/${id}/${action}`;
  const options = { method: 'POST' };
  if (action !== 'start') {
    options.headers = { 'Content-Type': 'application/json' };
    options.body = JSON.stringify({ force });
  }
  const response = await fetchWithToken(url, options);
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body?.message ?? `Aktion fehlgeschlagen (${response.status})`);
  }
  return response.json();
};

const performBulkAction = async (kind, force = false) => {
  const options = {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ force }),
  };
  if (kind === 'start-all') {
    delete options.body;
    delete options.headers['Content-Type'];
  }
  const response = await fetchWithToken(`/services/actions/${kind}`, options);
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body?.message ?? `Aktion fehlgeschlagen (${response.status})`);
  }
  return response.json();
};

const loadAll = async ({ background = false } = {}) => {
  if (isLoading) {
    return;
  }
  isLoading = true;
  if (!background) {
    showAlert('');
  }
  try {
    const [infoRes, servicesRes, metricsRes] = await Promise.all([
      fetchWithToken('/info'),
      fetchWithToken('/services'),
      fetchWithToken('/metrics'),
    ]);

    if (infoRes.ok) {
      updateMeta(await infoRes.json());
    }

    if (servicesRes.ok) {
      updateServices(await servicesRes.json());
    } else {
      servicesCache = new Map();
      svcBody.innerHTML = '<tr><td colspan="6">Fehler beim Laden der Services.</td></tr>';
      metaServices.textContent = `Fehler (${servicesRes.status})`;
    }

    if (metricsRes.ok) {
      const metrics = await metricsRes.json();
      if (!historyLoaded) {
        await fetchTelemetryHistory(currentHistoryRange, { background });
      }
      setUptimeBase(metrics.uptime_seconds ?? null);
      updateHealth(metrics.live, metrics.ready);
      recordMetricSample(metrics.counters ?? {});
      setServiceMetricsData(metrics.service_resources ?? []);
    } else {
      setUptimeBase(null);
      healthValue.textContent = 'unbekannt';
      healthNote.textContent = 'Telemetrie nicht verfügbar.';
      markTelemetryUnavailable(`Telemetrie nicht verfügbar (${metricsRes.status}).`);
      resetServiceMetrics('Telemetrie nicht verfügbar.');
    }

    if (!auditHistoryLoaded && !auditHistoryLoading) {
      fetchAuditHistory(currentAuditRange, { background: background || !document.hasFocus() }).catch(() => {});
    }
  } catch (error) {
    servicesCache = new Map();
    svcBody.innerHTML = '<tr><td colspan="6">Netzwerkfehler: Daten konnten nicht geladen werden.</td></tr>';
    metaServices.textContent = 'Fehler';
    showAlert(background ? 'Auto-Refresh fehlgeschlagen – Verbindung prüfen.' : 'Netzwerkfehler: Bitte Verbindung prüfen.');
    auditBody.innerHTML = '<tr><td colspan="6">Netzwerkfehler – keine Audit-Daten.</td></tr>';
    auditMeta.textContent = 'Fehler';
    auditHistoryLoaded = false;
    setUptimeBase(null);
    healthValue.textContent = 'unbekannt';
    healthNote.textContent = 'Telemetrie nicht verfügbar.';
    markTelemetryUnavailable('Telemetrie nicht verfügbar.');
    resetServiceMetrics('Telemetrie nicht verfügbar.');
  } finally {
    isLoading = false;
  }
};

const scheduleRefresh = () => {
  if (refreshHandle !== null) {
    window.clearInterval(refreshHandle);
  }
  refreshHandle = window.setInterval(() => {
    if (document.hidden) {
      return;
    }
    loadAll({ background: true });
  }, REFRESH_INTERVAL_MS);
};

tokenInput.addEventListener('change', () => {
  const token = currentToken();
  saveToken(token);
  setTokenUi(token);
  connectEventStream();
});

if (auditRefreshBtn) {
  auditRefreshBtn.addEventListener('click', () => {
    auditHistoryLoaded = false;
    historyLoaded = false;
    loadAll({ background: true });
  });
}

testButton.addEventListener('click', async () => {
  testButton.disabled = true;
  testButton.textContent = 'prüfe …';
  try {
    const response = await fetchWithToken('/services');
    if (response.ok) {
      tokenStatus.textContent = 'Token gültig – Zugriff erlaubt.';
    } else if (response.status === 401) {
      tokenStatus.textContent = 'Token ungültig – Autorisierung fehlgeschlagen (401).';
    } else if (response.status === 403) {
      tokenStatus.textContent = 'Token besitzt nicht ausreichende Rolle (403).';
    } else {
      tokenStatus.textContent = `Anfrage fehlgeschlagen (Status ${response.status}).`;
    }
  } catch (_) {
    tokenStatus.textContent = 'Netzwerkfehler – Anfrage konnte nicht gesendet werden.';
  } finally {
    setTimeout(() => {
      testButton.textContent = 'Token Test';
      testButton.disabled = currentToken() === '';
    }, 650);
  }
});

const handleBulk = async (kind) => {
  try {
    showAlert('');
    const destructive = kind !== 'start-all';
    if (destructive && !window.confirm('Aktion wirklich ausführen?')) {
      return;
    }
    const result = await performBulkAction(kind, destructive);
    const successes = result.results.filter((r) => r.status === 'success').length;
    const failures = result.results.length - successes;
    showAlert(
      `Aktion ${result.action} abgeschlossen: ${successes} erfolgreich, ${failures} fehlgeschlagen.`,
    );
    await loadAll();
  } catch (error) {
    showAlert(error.message || 'Bulk-Aktion fehlgeschlagen.');
  }
};

bulkStartBtn.addEventListener('click', () => handleBulk('start-all'));
bulkStopBtn.addEventListener('click', () => handleBulk('stop-all'));
bulkRestartBtn.addEventListener('click', () => handleBulk('restart-all'));

svcBody.addEventListener('click', async (event) => {
  const target = event.target;
  if (!(target instanceof HTMLElement)) {
    return;
  }
  const action = target.dataset.serviceAction;
  const id = target.dataset.serviceId;
  if (!action || !id) {
    return;
  }
  if (target.disabled) {
    return;
  }
  try {
    const destructive = action !== 'start';
    if (destructive && !window.confirm(`Aktion ${action} für ${id} wirklich ausführen?`)) {
      return;
    }
    target.disabled = true;
    await performServiceAction(id, action, destructive);
    showAlert(`Service ${id}: Aktion ${action} erfolgreich.`);
    await loadAll();
  } catch (error) {
    showAlert(error.message || `Aktion ${action} fehlgeschlagen.`);
  } finally {
    target.disabled = false;
  }
});

const init = async () => {
  const token = loadToken();
  tokenInput.value = token;
  setTokenUi(token);
  renderSeriesToggles();
  connectEventStream();
  await loadAll();
  scheduleRefresh();
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) {
      loadAll({ background: true });
    }
  });
  window.addEventListener('focus', () => loadAll({ background: true }));
};

init();
