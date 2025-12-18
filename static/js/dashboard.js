/* ============================================
   INTRO ANIMATION CONFIGURATION
   ============================================
   
   INTRO_MODE options:
   - "always" = Show intro on every page load
   - "once"   = Show intro only on first visit (saved in localStorage)
   - "none"   = Never show intro (disabled)
   
   RESET: resetFenrirIntro() dann F5
   ============================================ */

const INTRO_MODE = 'once'; // Options: "always", "once", "none"
const INTRO_STORAGE_KEY = 'fenrir_intro_seen';
const INTRO_SHOW_DURATION = 2500; // How long to show the intro (ms)

const initIntroAnimation = () => {
  const introOverlay = document.querySelector('[data-intro]');
  const introContent = document.querySelector('.intro-content');
  const appShell = document.querySelector('[data-app-shell]');
  
  if (!introOverlay || !appShell) {
    if (appShell) appShell.classList.add('app-visible');
    return;
  }
  
  // Check if intro is disabled via JS config or HTML attribute
  const isDisabledByHtml = introOverlay.dataset.introEnabled === 'false';
  const isDisabledByConfig = INTRO_MODE === 'none';
  
  if (isDisabledByHtml || isDisabledByConfig) {
    introOverlay.style.display = 'none';
    appShell.classList.add('app-visible');
    return;
  }
  
  // Determine intro mode (JS config takes priority over HTML attribute)
  const showAlways = INTRO_MODE === 'always';
  
  // Check if user already saw the intro (only if mode is "once")
  if (!showAlways && localStorage.getItem(INTRO_STORAGE_KEY)) {
    introOverlay.classList.add('intro-hidden');
    appShell.classList.add('app-visible');
    return;
  }
  
  // Reset intro state for "always" mode
  if (showAlways) {
    introOverlay.classList.remove('intro-hidden');
    introOverlay.style.opacity = '1';
    introOverlay.style.display = '';
    if (introContent) {
      introContent.style.opacity = '1';
      introContent.style.transform = '';
    }
  }
  
  // Run the intro - after showing, fade everything out nicely
  setTimeout(() => {
    // Fade out the intro content (logo + text) with scale
    if (introContent) {
      introContent.style.transition = 'opacity 0.6s ease, transform 0.6s ease';
      introContent.style.opacity = '0';
      introContent.style.transform = 'scale(0.9)';
    }
    
    // Then fade the overlay and show app
    setTimeout(() => {
      introOverlay.style.transition = 'opacity 0.5s ease';
      introOverlay.style.opacity = '0';
      
      // Show the app with build-up animation
      setTimeout(() => {
        introOverlay.classList.add('intro-hidden');
        appShell.classList.add('app-visible');
        // Only save to localStorage if mode is "once"
        if (!showAlways) {
          localStorage.setItem(INTRO_STORAGE_KEY, 'true');
        }
      }, 500);
    }, 400);
    
  }, INTRO_SHOW_DURATION);
};

// Reset function for testing
window.resetFenrirIntro = () => {
  localStorage.removeItem(INTRO_STORAGE_KEY);
  console.log('🐺 Intro reset! Drücke F5 zum Neuladen.');
  return 'F5 drücken!';
};

initIntroAnimation();

/* ============================================
   END INTRO ANIMATION
   ============================================ */

/* ============================================
   PERFORMANCE UTILITIES
   ============================================ */

// Debounce: delays execution until after wait ms of no calls
const debounce = (fn, wait = 150) => {
  let timeoutId = null;
  return (...args) => {
    if (timeoutId) clearTimeout(timeoutId);
    timeoutId = setTimeout(() => fn(...args), wait);
  };
};

// Throttle: limits execution to once per wait ms
const throttle = (fn, wait = 100) => {
  let lastTime = 0;
  let timeoutId = null;
  return (...args) => {
    const now = Date.now();
    const remaining = wait - (now - lastTime);
    if (remaining <= 0) {
      if (timeoutId) {
        clearTimeout(timeoutId);
        timeoutId = null;
      }
      lastTime = now;
      fn(...args);
    } else if (!timeoutId) {
      timeoutId = setTimeout(() => {
        lastTime = Date.now();
        timeoutId = null;
        fn(...args);
      }, remaining);
    }
  };
};

// RequestIdleCallback polyfill for non-critical tasks
const scheduleIdle = window.requestIdleCallback || ((cb) => setTimeout(cb, 1));

// Batch DOM reads/writes to avoid layout thrashing
const batchDOMUpdate = (fn) => {
  requestAnimationFrame(() => {
    requestAnimationFrame(fn);
  });
};

// Track page visibility for pausing updates
let isPageVisible = !document.hidden;
document.addEventListener('visibilitychange', () => {
  isPageVisible = !document.hidden;
  if (isPageVisible && lastRefreshAt && Date.now() - lastRefreshAt > REFRESH_INTERVAL_MS) {
    // Tab became visible and data is stale - refresh
    loadAll({ background: true });
  }
});

// Track which pages have been loaded (lazy loading)
const loadedPages = new Set(['overview']); // Overview loads by default
let currentVisiblePage = 'overview';

// Intersection Observer for lazy loading elements
const lazyObserver = new IntersectionObserver(
  (entries) => {
    entries.forEach((entry) => {
      if (entry.isIntersecting) {
        const el = entry.target;
        if (el.dataset.lazySrc) {
          el.src = el.dataset.lazySrc;
          delete el.dataset.lazySrc;
          lazyObserver.unobserve(el);
        }
        if (el.dataset.lazyLoad) {
          el.classList.add('lazy-loaded');
          lazyObserver.unobserve(el);
        }
      }
    });
  },
  { rootMargin: '50px', threshold: 0.1 }
);

/* ============================================
   END PERFORMANCE UTILITIES
   ============================================ */

/* ============================================
   CONFIGURATION & CONSTANTS
   ============================================ */

/** @constant {number} Auto-refresh interval in milliseconds */
const REFRESH_INTERVAL_MS = 15000;

/** @constant {number} Uptime ticker interval in milliseconds */
const UPTIME_TICK_MS = 1000;

// Telemetry Chart Configuration
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

// History Range Configuration
const HISTORY_RANGE_DEFAULT = '1h';
const HISTORY_RANGES = ['1h', '3h', '24h'];
const HISTORY_RETENTION_MS = 24 * 60 * 60 * 1000;

// Audit Configuration
const AUDIT_HISTORY_RANGE_DEFAULT = '1h';
const AUDIT_HISTORY_RANGES = ['1h', '3h', '24h'];
const AUDIT_HISTORY_LIMIT = 1000;

/* ============================================
   DOM ELEMENT REFERENCES
   ============================================ */

// Core UI Elements
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
const auditModal = document.querySelector('[data-audit-modal]');
const auditModalContent = document.querySelector('[data-audit-modal-content]');
const auditModalClose = document.querySelector('[data-audit-modal-close]');
const serviceModal = document.querySelector('[data-service-modal]');
const serviceModalContent = document.querySelector('[data-service-modal-content]');
const serviceModalClose = document.querySelector('[data-service-modal-close]');
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
const metricIoReadValue = document.querySelector('[data-metric-io-read]');
const metricIoWriteValue = document.querySelector('[data-metric-io-write]');

// Mini-chart elements
const miniChartCanvases = {
  cpu: document.querySelector('[data-mini-chart="cpu"]'),
  memory: document.querySelector('[data-mini-chart="memory"]'),
  'io-read': document.querySelector('[data-mini-chart="io-read"]'),
  'io-write': document.querySelector('[data-mini-chart="io-write"]'),
};
const miniChartValues = {
  cpu: document.querySelector('[data-chart-value="cpu"]'),
  memory: document.querySelector('[data-chart-value="memory"]'),
  'io-read': document.querySelector('[data-chart-value="io-read"]'),
  'io-write': document.querySelector('[data-chart-value="io-write"]'),
};
const miniChartMins = {
  cpu: document.querySelector('[data-chart-min="cpu"]'),
  memory: document.querySelector('[data-chart-min="memory"]'),
  'io-read': document.querySelector('[data-chart-min="io-read"]'),
  'io-write': document.querySelector('[data-chart-min="io-write"]'),
};
const miniChartMaxs = {
  cpu: document.querySelector('[data-chart-max="cpu"]'),
  memory: document.querySelector('[data-chart-max="memory"]'),
  'io-read': document.querySelector('[data-chart-max="io-read"]'),
  'io-write': document.querySelector('[data-chart-max="io-write"]'),
};
const miniChartAvgs = {
  cpu: document.querySelector('[data-chart-avg="cpu"]'),
  memory: document.querySelector('[data-chart-avg="memory"]'),
  'io-read': document.querySelector('[data-chart-avg="io-read"]'),
  'io-write': document.querySelector('[data-chart-avg="io-write"]'),
};

// Metric keys for each mini-chart
const MINI_CHART_METRICS = {
  cpu: 'process.cpu.usage_percent',
  memory: 'process.memory.resident_bytes',
  'io-read': 'process.io.read_bytes_per_sec',
  'io-write': 'process.io.write_bytes_per_sec',
};

// Colors for each mini-chart (Wolf Theme)
const MINI_CHART_COLORS = {
  cpu: '#6abadc',      // Cyan blue
  memory: '#7a9ab8',   // Steel blue
  'io-read': '#a8c8e8', // Light blue
  'io-write': '#5a8aaa', // Deep blue
};
const chartTooltip = document.querySelector('[data-chart-tooltip]');
const chartTooltipTime = document.querySelector('[data-tooltip-time]');
const chartTooltipBody = document.querySelector('[data-tooltip-body]');
const pageButtons = Array.from(document.querySelectorAll('[data-page-trigger]'));
const pageContainers = new Map(
  Array.from(document.querySelectorAll('[data-page]')).map((el) => [el.dataset.page, el]),
);
const historyRangeButtons = Array.from(document.querySelectorAll('[data-history-range]'));
const auditRangeButtons = Array.from(document.querySelectorAll('[data-audit-range]'));
const serviceSummaryCounters = new Map(
  Array.from(document.querySelectorAll('[data-service-count]')).map((el) => [el.dataset.serviceCount, el]),
);
const serviceSummaryEmpty = document.querySelector('[data-service-summary-empty]');
const serviceIncidentsList = document.querySelector('[data-service-incidents]');
const serviceIncidentsEmpty = document.querySelector('[data-service-incidents-empty]');
const serviceFilterInput = document.querySelector('[data-service-filter]');
const serviceFilterClear = document.querySelector('[data-service-filter-clear]');
const serviceStatusButtons = Array.from(document.querySelectorAll('[data-service-filter-status]'));
const modulesList = document.querySelector('[data-modules-list]');
const modulesMeta = document.querySelector('[data-modules-meta]');
const modulesEmpty = document.querySelector('[data-modules-empty]');
const auditPreviewList = document.querySelector('[data-audit-preview]');
const auditPreviewEmpty = document.querySelector('[data-audit-preview-empty]');
const refreshNote = document.querySelector('[data-refresh-note]');
const modulesEmptyDefault =
  (modulesEmpty && modulesEmpty.textContent && modulesEmpty.textContent.trim()) ||
  'Keine Module installiert.';

// Storage Keys
const TOKEN_KEY = 'fenrir-control-plane-token';
const PAGE_STORAGE_KEY = 'fenrir-control-plane-page';

/* ============================================
   STATE MANAGEMENT
   ============================================ */

// Timer & Loading State
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
// Fenrir Wolf Theme - slate blues
const METRIC_COLORS = [
  '#6abadc', // Cyan blue
  '#7a9ab8', // Steel blue
  '#a8c8e8', // Light blue
  '#5a8aaa', // Deep blue
  '#8ab4d4', // Sky blue
  '#4a7a9a', // Dark teal
];
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
let serviceFilterValue = '';
let serviceStatusFilter = 'all';
let serviceTableEmptyMessage = 'Keine Services registriert.';
let serviceMetaOverride = null;
let modulesCache = [];
let lastRefreshAt = null;

// DOM Cache - prevents flickering on refresh
let lastServicesTableHtml = '';
let lastIncidentsHtml = '';
let lastModulesHtml = '';
let lastAuditTableHtml = '';
let lastAuditPreviewHtml = '';

/* ============================================
   STORAGE & TOKEN FUNCTIONS
   ============================================ */

/**
 * Load authentication token from localStorage
 * @returns {string} The stored token or empty string
 */
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

/** @returns {string} Current token from input field */
const currentToken = () => tokenInput.value.trim();

/* ============================================
   UI STATE FUNCTIONS
   ============================================ */

/**
 * Update active state of history range buttons
 * @param {string} range - Active range ('1h', '3h', '24h')
 */
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

/* ============================================
   API & AUTHENTICATION
   ============================================ */

/**
 * Generate authorization headers for API requests
 * @returns {Object} Headers object with Authorization if token exists
 */
const authHeaders = () => {
  const token = currentToken();
  if (!token) {
    return {};
  }
  return {
    Authorization: token.startsWith('Bearer ') ? token : `Bearer ${token}`,
  };
};

/**
 * Update token input UI state
 * @param {string} token - Current token value
 */
const setTokenUi = (token) => {
  if (!token) {
    tokenStatus.textContent = 'Token nicht gesetzt – Anfragen erfolgen ohne Auth.';
    testButton.disabled = true;
  } else {
    tokenStatus.textContent = 'Token aktiv – geschützte Endpunkte verwenden nun Autorisierung.';
    testButton.disabled = false;
  }
};

const updateRefreshNote = () => {
  if (!refreshNote) {
    return;
  }
  if (!lastRefreshAt) {
    refreshNote.textContent = 'Noch nicht aktualisiert';
    return;
  }
  const absolute = new Date(lastRefreshAt).toLocaleTimeString('de-DE', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
  refreshNote.textContent = `Aktualisiert ${formatRelativeTime(lastRefreshAt)} (${absolute})`;
};

/**
 * Show or hide alert message
 * @param {string} message - Alert message (empty to hide)
 */
const showAlert = (message) => {
  if (!message) {
    alertEl.dataset.visible = 'false';
    alertEl.textContent = '';
    return;
  }
  alertEl.dataset.visible = 'true';
  alertEl.textContent = message;
};

/* ============================================
   FORMATTING UTILITIES
   ============================================ */

/**
 * Format seconds as human-readable duration
 * @param {number} seconds - Duration in seconds
 * @returns {string} Formatted duration (e.g., "2d 5h", "45m")
 */
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

/* ============================================
   CHART SERIES CONFIGURATION
   ============================================ */

/**
 * Render series toggle checkboxes for chart
 */
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

/* ============================================
   SERVICE METRICS RENDERING
   ============================================ */

/**
 * Render service resource metrics table
 * @param {string} [emptyMessage] - Message to show when no data
 */
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

  const readRate = Number(counters['process.io.read_bytes_per_sec']);
  const writeRate = Number(counters['process.io.write_bytes_per_sec']);
  
  if (metricIoValue) {
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
  
  // Update separate I/O values for new layout
  if (metricIoReadValue && Number.isFinite(readRate)) {
    metricIoReadValue.textContent = formatThroughput(readRate);
  }
  if (metricIoWriteValue && Number.isFinite(writeRate)) {
    metricIoWriteValue.textContent = formatThroughput(writeRate);
  }
};

/* ============================================
   CHART RENDERING
   ============================================ */

/**
 * Render chart legend with metric entries
 * @param {Array} entries - Array of metric entries with key, label, color, value
 */
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
    metricListBody.innerHTML = '<div class="metric-row placeholder"><span>Keine Telemetriedaten</span></div>';
    return;
  }

  const pinned = PINNED_METRIC_KEYS
    .map((key) => [key, Number(counters?.[key])])
    .filter(([, value]) => Number.isFinite(value));
  const seen = new Set(pinned.map(([key]) => key));

  const dynamic = entries
    .filter(([key]) => !seen.has(key))
    .sort((a, b) => b[1] - a[1]);

  const combined = [...pinned, ...dynamic].slice(0, 12);
  metricListBody.innerHTML = combined
    .map(([name, value]) => {
      const label = escapeHtml(METRIC_LABEL_OVERRIDES[name] ?? formatMetricLabel(name));
      const valueText = escapeHtml(formatTooltipValue(name, value));
      return `<div class="metric-row"><span>${label}</span><span>${valueText}</span></div>`;
    })
    .join('');
};

const setupCanvasDPI = (canvas, ctx) => {
  const dpr = window.devicePixelRatio || 1;
  const rect = canvas.getBoundingClientRect();
  const width = rect.width || 960;
  const height = rect.height || 320;
  
  canvas.width = width * dpr;
  canvas.height = height * dpr;
  canvas.style.width = width + 'px';
  canvas.style.height = height + 'px';
  
  ctx.scale(dpr, dpr);

  return { width, height, dpr };
};

// ============================================
// SMOOTH BEZIER CURVE HELPER
// Creates smooth rounded lines instead of sharp edges
// ============================================

// Maximum points for smooth rendering - fewer points = smoother curves
const MAX_CHART_POINTS = 50;

/**
 * Downsample points using LTTB-like algorithm to preserve visual shape
 * @param {Array<{x: number, y: number}>} points - Input points
 * @param {number} targetCount - Target number of points
 * @returns {Array<{x: number, y: number}>} Downsampled points
 */
const downsamplePoints = (points, targetCount) => {
  if (points.length <= targetCount) return points;
  
  const result = [];
  const bucketSize = (points.length - 2) / (targetCount - 2);
  
  // Always keep first point
  result.push(points[0]);
  
  for (let i = 0; i < targetCount - 2; i++) {
    const bucketStart = Math.floor(i * bucketSize) + 1;
    const bucketEnd = Math.min(Math.floor((i + 1) * bucketSize) + 1, points.length - 1);
    
    // Find the point with max area (most significant point in bucket)
    let maxArea = -1;
    let maxIndex = bucketStart;
    
    const prevPoint = result[result.length - 1];
    
    // Calculate average point in next bucket for area calculation
    const nextBucketStart = Math.min(Math.floor((i + 1) * bucketSize) + 1, points.length - 1);
    const nextBucketEnd = Math.min(Math.floor((i + 2) * bucketSize) + 1, points.length);
    let avgX = 0, avgY = 0, avgCount = 0;
    for (let j = nextBucketStart; j < nextBucketEnd; j++) {
      avgX += points[j].x;
      avgY += points[j].y;
      avgCount++;
    }
    if (avgCount > 0) {
      avgX /= avgCount;
      avgY /= avgCount;
    } else {
      avgX = points[points.length - 1].x;
      avgY = points[points.length - 1].y;
    }
    
    // Find point with largest triangle area
    for (let j = bucketStart; j < bucketEnd; j++) {
      const area = Math.abs(
        (prevPoint.x - avgX) * (points[j].y - prevPoint.y) -
        (prevPoint.x - points[j].x) * (avgY - prevPoint.y)
      );
      if (area > maxArea) {
        maxArea = area;
        maxIndex = j;
      }
    }
    
    result.push(points[maxIndex]);
  }
  
  // Always keep last point
  result.push(points[points.length - 1]);
  
  return result;
};

/**
 * Draw a smooth curve using quadratic bezier curves through midpoints
 * This creates genuinely smooth, rounded lines
 * @param {CanvasRenderingContext2D} ctx - Canvas context
 * @param {Array<{x: number, y: number}>} rawPoints - Array of points
 * @param {boolean} closePath - Whether to close the path (for area fill)
 * @param {number} baseY - Base Y coordinate for closing path (area fill)
 */
const drawSmoothCurve = (ctx, rawPoints, closePath = false, baseY = 0) => {
  if (rawPoints.length < 2) return;
  
  // Downsample if too many points for smooth rendering
  const points = rawPoints.length > MAX_CHART_POINTS 
    ? downsamplePoints(rawPoints, MAX_CHART_POINTS) 
    : rawPoints;
  
  ctx.beginPath();
  
  if (closePath) {
    ctx.moveTo(points[0].x, baseY);
    ctx.lineTo(points[0].x, points[0].y);
  } else {
    ctx.moveTo(points[0].x, points[0].y);
  }
  
  // For only 2 points, draw a straight line
  if (points.length === 2) {
    ctx.lineTo(points[1].x, points[1].y);
    if (closePath) {
      ctx.lineTo(points[1].x, baseY);
      ctx.closePath();
    }
    return;
  }
  
  // Draw smooth curve using quadratic bezier through midpoints
  // Start with line to first midpoint
  let midX = (points[0].x + points[1].x) / 2;
  let midY = (points[0].y + points[1].y) / 2;
  ctx.lineTo(midX, midY);
  
  // Draw quadratic curves through each point
  for (let i = 1; i < points.length - 1; i++) {
    const nextMidX = (points[i].x + points[i + 1].x) / 2;
    const nextMidY = (points[i].y + points[i + 1].y) / 2;
    
    // Quadratic curve with control point at the actual data point
    ctx.quadraticCurveTo(points[i].x, points[i].y, nextMidX, nextMidY);
  }
  
  // Line to last point
  ctx.lineTo(points[points.length - 1].x, points[points.length - 1].y);
  
  if (closePath) {
    ctx.lineTo(points[points.length - 1].x, baseY);
    ctx.closePath();
  }
};

// ============================================
// MINI-CHARTS RENDERING (Grafana style)
// ============================================

const miniChartContexts = {};

// Store chart data for hover functionality
const miniChartData = {};

// Hover state
let activeHoverChart = null;
let hoverIndex = -1;

// Create tooltip element for mini charts
const miniChartTooltip = document.createElement('div');
miniChartTooltip.className = 'mini-chart-tooltip';
miniChartTooltip.style.cssText = `
  position: fixed;
  display: none;
  padding: 8px 12px;
  background: rgba(15, 23, 35, 0.95);
  border: 1px solid rgba(90, 122, 154, 0.3);
  border-radius: 8px;
  font-family: 'JetBrains Mono', monospace;
  font-size: 11px;
  color: #e8eef4;
  pointer-events: none;
  z-index: 1000;
  backdrop-filter: blur(8px);
  box-shadow: 0 4px 20px rgba(0, 0, 0, 0.4);
`;
document.body.appendChild(miniChartTooltip);

const renderMiniChart = (chartId) => {
  const canvas = miniChartCanvases[chartId];
  if (!canvas) return;
  
  if (!miniChartContexts[chartId]) {
    miniChartContexts[chartId] = canvas.getContext('2d', { alpha: true });
  }
  const ctx = miniChartContexts[chartId];
  
  // Get metric key for this chart
  const metricKey = MINI_CHART_METRICS[chartId];
  const color = MINI_CHART_COLORS[chartId];
  
  // Setup canvas DPI
  const dpr = window.devicePixelRatio || 1;
  const rect = canvas.getBoundingClientRect();
  canvas.width = rect.width * dpr;
  canvas.height = rect.height * dpr;
  ctx.scale(dpr, dpr);
  
  const width = rect.width;
  const height = rect.height;
  const padding = { top: 8, right: 8, bottom: 20, left: 45 };
  const plotWidth = width - padding.left - padding.right;
  const plotHeight = height - padding.top - padding.bottom;
  
  // Clear canvas (transparent - no background, blends with card)
  ctx.clearRect(0, 0, width, height);
  
  // Get data for this metric
  const history = metricHistory;
  if (!history || history.length === 0) {
    ctx.fillStyle = 'rgba(122, 154, 184, 0.4)';
    ctx.font = '11px "JetBrains Mono", monospace';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText('Keine Daten', width / 2, height / 2);
    return;
  }
  
  // Extract values
  const values = [];
  const timestamps = [];
  history.forEach((sample) => {
    const val = sample.counters[metricKey];
    if (Number.isFinite(val)) {
      values.push(val);
      timestamps.push(sample.timestamp);
    }
  });
  
  if (values.length === 0) {
    ctx.fillStyle = 'rgba(204, 204, 220, 0.3)';
    ctx.font = '11px "JetBrains Mono", monospace';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText('No data', width / 2, height / 2);
    return;
  }
  
  // Calculate min/max/avg
  const minVal = Math.min(...values);
  const maxVal = Math.max(...values);
  const avgVal = values.reduce((a, b) => a + b, 0) / values.length;
  const currentVal = values[values.length - 1];
  
  // Update stats display
  const formatVal = (v) => {
    if (chartId === 'cpu') return formatPercent(v, 1);
    if (chartId === 'memory') return formatBytes(v);
    return formatBytes(v) + '/s';
  };
  
  // Shorter format for Y-axis
  const formatAxisVal = (v) => {
    if (chartId === 'cpu') return Math.round(v) + '%';
    if (chartId === 'memory') {
      if (v >= 1073741824) return (v / 1073741824).toFixed(1) + 'G';
      if (v >= 1048576) return (v / 1048576).toFixed(0) + 'M';
      if (v >= 1024) return (v / 1024).toFixed(0) + 'K';
      return Math.round(v) + 'B';
    }
    if (v >= 1048576) return (v / 1048576).toFixed(1) + 'M/s';
    if (v >= 1024) return (v / 1024).toFixed(0) + 'K/s';
    return Math.round(v) + 'B/s';
  };
  
  if (miniChartValues[chartId]) miniChartValues[chartId].textContent = formatVal(currentVal);
  if (miniChartMins[chartId]) miniChartMins[chartId].textContent = formatVal(minVal);
  if (miniChartMaxs[chartId]) miniChartMaxs[chartId].textContent = formatVal(maxVal);
  if (miniChartAvgs[chartId]) miniChartAvgs[chartId].textContent = formatVal(avgVal);
  
  // Calculate Y axis range with nice rounding
  const yMin = 0;
  let yMax = maxVal * 1.15 || 100;
  // Round to nice numbers
  if (chartId === 'cpu') {
    yMax = Math.ceil(yMax / 10) * 10;
    if (yMax < 10) yMax = 10;
    if (yMax > 100) yMax = Math.ceil(yMax / 25) * 25;
  }
  const yRange = yMax - yMin || 1;
  
  // Draw subtle grid lines
  ctx.strokeStyle = 'rgba(90, 122, 154, 0.08)';
  ctx.lineWidth = 1;
  
  const gridLines = 4;
  for (let i = 1; i < gridLines; i++) {
    const y = Math.round(padding.top + (plotHeight / gridLines) * i) + 0.5;
    ctx.beginPath();
    ctx.moveTo(padding.left, y);
    ctx.lineTo(width - padding.right, y);
    ctx.stroke();
  }
  
  // Y-axis labels
  ctx.fillStyle = 'rgba(140, 165, 190, 0.6)';
  ctx.font = '500 9px "JetBrains Mono", monospace';
  ctx.textAlign = 'right';
  ctx.textBaseline = 'middle';
  
  for (let i = 0; i <= gridLines; i++) {
    const ratio = 1 - (i / gridLines);
    const val = yMin + yRange * ratio;
    const y = padding.top + (plotHeight / gridLines) * i;
    ctx.fillText(formatAxisVal(val), padding.left - 8, y);
  }
  
  // X-axis time labels
  ctx.fillStyle = 'rgba(140, 165, 190, 0.5)';
  ctx.font = '500 9px "JetBrains Mono", monospace';
  ctx.textBaseline = 'top';
  
  if (timestamps.length > 0) {
    const firstTime = new Date(timestamps[0]).toLocaleTimeString('de-DE', { hour: '2-digit', minute: '2-digit' });
    const lastTime = new Date(timestamps[timestamps.length - 1]).toLocaleTimeString('de-DE', { hour: '2-digit', minute: '2-digit' });
    ctx.textAlign = 'left';
    ctx.fillText(firstTime, padding.left, height - padding.bottom + 6);
    ctx.textAlign = 'right';
    ctx.fillText(lastTime, width - padding.right, height - padding.bottom + 6);
  }
  
  // Draw area fill
  const stepX = plotWidth / Math.max(values.length - 1, 1);
  
  // Calculate points
  const points = values.map((val, i) => ({
    x: padding.left + stepX * i,
    y: padding.top + plotHeight * (1 - (val - yMin) / yRange)
  }));
  
  // Area gradient fill with smooth curve
  ctx.save();
  const gradient = ctx.createLinearGradient(0, padding.top, 0, height - padding.bottom);
  gradient.addColorStop(0, withAlpha(color, 0.2));
  gradient.addColorStop(0.5, withAlpha(color, 0.08));
  gradient.addColorStop(1, 'transparent');
  ctx.fillStyle = gradient;
  
  drawSmoothCurve(ctx, points, true, height - padding.bottom);
  ctx.fill();
  ctx.restore();
  
  // Clean main line (no glow)
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = 2;
  ctx.lineJoin = 'round';
  ctx.lineCap = 'round';
  drawSmoothCurve(ctx, points, false);
  ctx.stroke();
  ctx.restore();
  
  // Store data for hover functionality
  miniChartData[chartId] = {
    values,
    timestamps,
    points,
    padding,
    width,
    height,
    color,
    formatVal,
    yMin,
    yRange,
    plotHeight
  };
  
  // Draw hover overlay if this chart is being hovered
  if (activeHoverChart === chartId && hoverIndex >= 0 && hoverIndex < points.length) {
    drawHoverOverlay(ctx, chartId, hoverIndex);
  }
};

// Draw hover overlay (vertical line + dot + crosshair)
const drawHoverOverlay = (ctx, chartId, index) => {
  const data = miniChartData[chartId];
  if (!data || !data.points[index]) return;
  
  const point = data.points[index];
  const { padding, height, color } = data;
  
  // Vertical line
  ctx.save();
  ctx.strokeStyle = 'rgba(140, 165, 190, 0.3)';
  ctx.lineWidth = 1;
  ctx.setLineDash([4, 4]);
  ctx.beginPath();
  ctx.moveTo(point.x, padding.top);
  ctx.lineTo(point.x, height - padding.bottom);
  ctx.stroke();
  ctx.restore();
  
  // Highlight dot
  ctx.save();
  ctx.beginPath();
  ctx.arc(point.x, point.y, 5, 0, Math.PI * 2);
  ctx.fillStyle = 'rgba(15, 23, 35, 0.8)';
  ctx.fill();
  ctx.beginPath();
  ctx.arc(point.x, point.y, 4, 0, Math.PI * 2);
  ctx.fillStyle = color;
  ctx.fill();
  ctx.restore();
};

const renderAllMiniCharts = () => {
  Object.keys(miniChartCanvases).forEach((chartId) => {
    renderMiniChart(chartId);
  });
};

// Mini chart hover handlers
const handleMiniChartHover = (chartId, e) => {
  const canvas = miniChartCanvases[chartId];
  const data = miniChartData[chartId];
  if (!canvas || !data || !data.points.length) return;
  
  const rect = canvas.getBoundingClientRect();
  const mouseX = e.clientX - rect.left;
  
  // Find closest point
  let closestIndex = 0;
  let closestDist = Infinity;
  
  data.points.forEach((point, i) => {
    const dist = Math.abs(point.x - mouseX);
    if (dist < closestDist) {
      closestDist = dist;
      closestIndex = i;
    }
  });
  
  // Only update if index changed
  if (activeHoverChart !== chartId || hoverIndex !== closestIndex) {
    activeHoverChart = chartId;
    hoverIndex = closestIndex;
    renderMiniChart(chartId);
    
    // Update tooltip
    const point = data.points[closestIndex];
    const value = data.values[closestIndex];
    const timestamp = data.timestamps[closestIndex];
    const time = new Date(timestamp).toLocaleTimeString('de-DE', { 
      hour: '2-digit', 
      minute: '2-digit',
      second: '2-digit'
    });
    const date = new Date(timestamp).toLocaleDateString('de-DE', {
      day: '2-digit',
      month: '2-digit'
    });
    
    miniChartTooltip.innerHTML = `
      <div style="color: ${data.color}; font-weight: 600; font-size: 13px; margin-bottom: 4px;">
        ${data.formatVal(value)}
      </div>
      <div style="color: rgba(140, 165, 190, 0.8); font-size: 10px;">
        ${date} · ${time}
      </div>
    `;
    miniChartTooltip.style.display = 'block';
    
    // Position tooltip
    const tooltipRect = miniChartTooltip.getBoundingClientRect();
    let tooltipX = e.clientX + 15;
    let tooltipY = e.clientY - tooltipRect.height / 2;
    
    // Keep tooltip in viewport
    if (tooltipX + tooltipRect.width > window.innerWidth - 10) {
      tooltipX = e.clientX - tooltipRect.width - 15;
    }
    if (tooltipY < 10) tooltipY = 10;
    if (tooltipY + tooltipRect.height > window.innerHeight - 10) {
      tooltipY = window.innerHeight - tooltipRect.height - 10;
    }
    
    miniChartTooltip.style.left = tooltipX + 'px';
    miniChartTooltip.style.top = tooltipY + 'px';
  }
};

const handleMiniChartLeave = (chartId) => {
  if (activeHoverChart === chartId) {
    activeHoverChart = null;
    hoverIndex = -1;
    miniChartTooltip.style.display = 'none';
    renderMiniChart(chartId);
  }
};

// Attach hover listeners to mini charts
Object.keys(miniChartCanvases).forEach((chartId) => {
  const canvas = miniChartCanvases[chartId];
  if (canvas) {
    canvas.addEventListener('mousemove', (e) => handleMiniChartHover(chartId, e));
    canvas.addEventListener('mouseleave', () => handleMiniChartLeave(chartId));
    canvas.style.cursor = 'crosshair';
  }
});

// ============================================
// MAIN CHART RENDERING (kept for compatibility)
// ============================================

const renderMetricChart = () => {
  // Also render mini-charts
  renderAllMiniCharts();
  
  if (!metricChartCanvas) {
    return;
  }
  if (!metricCtx) {
    metricCtx = metricChartCanvas.getContext('2d', { alpha: true });
  }
  if (!metricCtx) {
    return;
  }
  
  // Setup DPI scaling for sharp rendering
  const { width, height } = setupCanvasDPI(metricChartCanvas, metricCtx);
  
  chartHoverState = null;
  const history = getHistoryForChart();
  const ctx = metricCtx;
  
  // Clear with proper dimensions
  ctx.clearRect(0, 0, width, height);
  
  if (history.length === 0) {
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

  const padding = 56;
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

  // ===== SIMPLE DARK BACKGROUND =====
  ctx.save();
  ctx.fillStyle = '#111318';
  ctx.fillRect(0, 0, width, height);
  ctx.restore();

  // ===== CLEAN GRID (Grafana style) =====
  ctx.save();
  const gridY = 4;
  
  // Horizontal grid lines
  ctx.strokeStyle = 'rgba(255, 255, 255, 0.06)';
  ctx.lineWidth = 1;
  
  for (let i = 0; i <= gridY; i++) {
    const y = padding + (plotHeight / gridY) * i;
    ctx.beginPath();
    ctx.moveTo(padding, y);
    ctx.lineTo(width - padding, y);
    ctx.stroke();
  }
  ctx.restore();

  // ===== DRAW DATA SERIES =====
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

    // ===== AREA FILL (Grafana style with smooth curve) =====
    if (pathPoints.length > 1) {
      ctx.save();
      const areaGradient = ctx.createLinearGradient(0, padding, 0, height - padding);
      areaGradient.addColorStop(0, withAlpha(color, 0.4));
      areaGradient.addColorStop(1, withAlpha(color, 0.05));
      
      ctx.fillStyle = areaGradient;
      drawSmoothCurve(ctx, pathPoints, true, height - padding);
      ctx.fill();
      ctx.restore();
    }

    // ===== MAIN LINE (smooth bezier curve) =====
    ctx.save();
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.5;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';
    drawSmoothCurve(ctx, pathPoints, false);
    ctx.stroke();
    ctx.restore();

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

  // ===== Y-AXIS LABELS (Grafana style) =====
  ctx.save();
  ctx.font = '400 10px "JetBrains Mono", monospace';
  ctx.fillStyle = 'rgba(204, 204, 220, 0.65)';
  ctx.textAlign = 'right';
  ctx.textBaseline = 'middle';
  
  const yLabels = 4;
  for (let i = 0; i <= yLabels; i++) {
    const ratio = 1 - (i / yLabels);
    const value = primaryAxis.min + (primaryAxis.max - primaryAxis.min) * ratio;
    const y = padding + (plotHeight / yLabels) * i;
    const label = formatAxisValue(primaryAxisKey, value);
    if (label) {
      ctx.fillText(label, padding - 8, y);
    }
  }
  ctx.restore();

  // ===== X-AXIS TIME LABELS =====
  ctx.save();
  ctx.font = '400 10px "JetBrains Mono", monospace';
  ctx.fillStyle = 'rgba(204, 204, 220, 0.65)';
  ctx.textBaseline = 'top';
  
  if (Number.isFinite(firstTimestamp)) {
    ctx.textAlign = 'left';
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
  // Use CSS dimensions for hover calculation (not DPI-scaled canvas dimensions)
  let canvasX = event.clientX - rect.left;
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
    renderMetricList({});
    renderServiceSummary({});
    return;
  }
  const counters = collectServiceMetrics();
  renderMetricList(counters);
  renderServiceSummary(counters);
};

/* ============================================
   SERVICE RENDERING
   ============================================ */

/**
 * Render service status summary counters
 * @param {Object} counters - Service status counts
 */
const renderServiceSummary = (counters = {}) => {
  if (serviceSummaryCounters.size === 0) {
    return;
  }
  let hasData = false;
  serviceSummaryCounters.forEach((element, key) => {
    if (!element) {
      return;
    }
    const value = Number(counters[key]);
    if (Number.isFinite(value)) {
      element.textContent = value.toString();
      if (value > 0) {
        hasData = true;
      }
    } else {
      element.textContent = '0';
    }
  });
  if (serviceSummaryEmpty) {
    serviceSummaryEmpty.dataset.visible = hasData ? 'false' : 'true';
  }
};

const INCIDENT_STATUS_SET = new Set(['failed', 'degraded', 'starting']);

const renderServiceIncidents = () => {
  if (!serviceIncidentsList) {
    return;
  }
  const incidents = Array.from(servicesCache.values()).filter((svc) => {
    const status = (svc.status || '').toLowerCase();
    return INCIDENT_STATUS_SET.has(status);
  });
  if (incidents.length === 0) {
    if (lastIncidentsHtml !== '') {
      serviceIncidentsList.innerHTML = '';
      lastIncidentsHtml = '';
    }
    if (serviceIncidentsEmpty && serviceIncidentsEmpty.dataset.visible !== 'true') {
      serviceIncidentsEmpty.dataset.visible = 'true';
    }
    return;
  }
  const limited = incidents.slice(0, 5);
  const newHtml = limited
    .map((svc) => {
      const displayName = escapeHtml(svc.name ?? svc.id ?? 'unbekannt');
      const identifier = escapeHtml(svc.id ?? '—');
      const status = escapeHtml(svc.status ?? 'unknown');
      return `
        <li class="incident-item">
          <div>
            <strong>${displayName}</strong>
            <span>${identifier}</span>
          </div>
          <span class="${statusClass(svc.status ?? 'unknown')}">${status}</span>
        </li>
      `;
    })
    .join('');
  
  // Only update DOM if content actually changed
  if (newHtml !== lastIncidentsHtml) {
    serviceIncidentsList.innerHTML = newHtml;
    lastIncidentsHtml = newHtml;
  }
  if (serviceIncidentsEmpty && serviceIncidentsEmpty.dataset.visible !== 'false') {
    serviceIncidentsEmpty.dataset.visible = 'false';
  }
};

/* ============================================
   MODULES RENDERING
   ============================================ */

const showModulesEmpty = (message = modulesEmptyDefault) => {
  if (modulesEmpty) {
    modulesEmpty.textContent = message;
    if (modulesEmpty.dataset.visible !== 'true') {
      modulesEmpty.dataset.visible = 'true';
    }
  }
  if (modulesList && lastModulesHtml !== '') {
    modulesList.innerHTML = '';
    lastModulesHtml = '';
  }
};

const renderModulesList = () => {
  if (!modulesList) {
    return;
  }
  if (!modulesCache || modulesCache.length === 0) {
    showModulesEmpty(modulesEmptyDefault);
    return;
  }
  const newHtml = modulesCache.slice(0, 6)
    .map((entry) => {
      const manifest = entry.manifest || {};
      const title = manifest.title || manifest.id || 'unbekanntes Modul';
      const id = manifest.id || 'n/a';
      const version = manifest.version || '–';
      const description = manifest.description || entry.path || '';
      const installed = entry.installed_at
        ? `installiert ${formatRelativeTime(Date.parse(entry.installed_at))}`
        : 'Installationszeit unbekannt';
      return `
        <li class="module-entry">
          <div>
            <strong>${escapeHtml(title)}</strong>
            <span class="module-id">${escapeHtml(id)}</span>
          </div>
          <div class="module-meta">
            <span>${escapeHtml(installed)}</span>
            <span>${escapeHtml(version)}</span>
          </div>
          ${
            description
              ? `<div class="module-description">${escapeHtml(description)}</div>`
              : ''
          }
        </li>
      `;
    })
    .join('');
  
  // Only update DOM if content changed
  if (newHtml !== lastModulesHtml) {
    modulesList.innerHTML = newHtml;
    lastModulesHtml = newHtml;
  }
  if (modulesEmpty && modulesEmpty.dataset.visible !== 'false') {
    modulesEmpty.dataset.visible = 'false';
  }
};

const updateModulesBoard = (payload) => {
  modulesCache = Array.isArray(payload?.modules) ? payload.modules : [];
  if (modulesMeta) {
    modulesMeta.textContent = modulesCache.length
      ? `${modulesCache.length} Module installiert`
      : 'Keine Module installiert';
  }
  if (modulesCache.length === 0) {
    showModulesEmpty(modulesEmptyDefault);
    return;
  }
  renderModulesList();
};

const setModulesError = (message) => {
  modulesCache = [];
  if (modulesMeta) {
    modulesMeta.textContent = message;
  }
  showModulesEmpty(message);
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
  currentVisiblePage = target;
  
  pageContainers.forEach((element, key) => {
    const isVisible = key === target;
    if (isVisible) {
      // Remove animation class and hide first
      element.classList.remove('page-entering');
      element.dataset.visible = 'false';
      // Force reflow to reset state
      void element.offsetWidth;
      // Show page
      element.dataset.visible = 'true';
      // Trigger animation in next frame
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          element.classList.add('page-entering');
        });
      });
    } else {
      element.dataset.visible = 'false';
      element.classList.remove('page-entering');
    }
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
  
  // Lazy load page-specific data
  if (target === 'telemetry') {
    window.requestAnimationFrame(() => renderMetricChart());
    if (!historyLoaded && !historyLoading) {
      fetchTelemetryHistory(currentHistoryRange, { background: true }).catch(() => {});
    }
  } else if (target === 'audit') {
    // Load audit data only when audit page is visited
    if (!auditHistoryLoaded && !auditHistoryLoading) {
      fetchAuditHistory(currentAuditRange, { background: true }).catch(() => {});
    }
  } else {
    hideChartTooltip();
  }
  
  // Track loaded pages for future optimization
  loadedPages.add(target);
};

resetTelemetrySummary();
setChartEmpty(true, 'Noch keine Telemetriedaten verfügbar.');
updateRefreshNote();
if (refreshNote) {
  window.setInterval(updateRefreshNote, 15000);
}

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

if (serviceFilterInput) {
  // Debounced filter for better performance
  const debouncedFilter = debounce((value) => setServiceFilterValue(value), 150);
  serviceFilterInput.addEventListener('input', (event) => {
    debouncedFilter(event.target.value || '');
  });
  serviceFilterInput.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      serviceFilterInput.value = '';
      setServiceFilterValue(''); // Immediate clear
    }
  });
}

if (serviceFilterClear) {
  serviceFilterClear.addEventListener('click', () => {
    if (serviceFilterInput) {
      serviceFilterInput.value = '';
      serviceFilterInput.focus();
    }
    setServiceFilterValue('');
  });
}

if (serviceStatusButtons.length > 0) {
  serviceStatusButtons.forEach((button) => {
    button.addEventListener('click', () => {
      const status = button.dataset.serviceFilterStatus || 'all';
      setServiceStatusFilter(status);
    });
  });
  setServiceStatusFilter(serviceStatusFilter);
} else {
  renderServicesTable();
}

updateServiceFilterClearState();

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

/* ============================================
   AUDIT RENDERING
   ============================================ */

/**
 * Render actor display for audit events
 * @param {string} actor - Actor identifier
 * @returns {string} HTML for actor display
 */
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
    const emptyHtml = '<tr><td colspan="4">Keine Audit-Ereignisse vorhanden.</td></tr>';
    if (lastAuditTableHtml !== emptyHtml) {
      auditBody.innerHTML = emptyHtml;
      lastAuditTableHtml = emptyHtml;
    }
    auditMeta.textContent = `0 Einträge · Range ${currentAuditRange}`;
    renderAuditPreview();
    return;
  }
  auditMeta.textContent = `${auditCache.length} Einträge · Range ${currentAuditRange}`;
  const newHtml = auditCache
    .map((event, index) => {
      const outcomeClass = event.outcome === 'success'
        ? 'pill success'
        : event.outcome === 'denied'
        ? 'pill denied'
        : 'pill failure';
      const shortTarget = (event.target || '–').length > 40 
        ? event.target.substring(0, 40) + '…' 
        : (event.target || '–');
      return `
        <tr data-audit-index="${index}">
          <td>${formatTimestamp(event.timestamp)}</td>
          <td class="action-cell">${escapeHtml(event.action || '–')}</td>
          <td class="target-cell" title="${escapeHtml(event.target || '')}">${escapeHtml(shortTarget)}</td>
          <td><span class="${outcomeClass}">${escapeHtml(event.outcome || '–')}</span></td>
        </tr>
      `;
    })
    .join('');
  
  // Only update DOM if content changed
  if (newHtml !== lastAuditTableHtml) {
    auditBody.innerHTML = newHtml;
    lastAuditTableHtml = newHtml;
    attachAuditRowListeners();
  }
  renderAuditPreview();
};

const attachAuditRowListeners = () => {
  const rows = auditBody.querySelectorAll('tr[data-audit-index]');
  rows.forEach((row) => {
    row.addEventListener('click', () => {
      const index = parseInt(row.dataset.auditIndex, 10);
      const event = auditCache[index];
      if (event) {
        showAuditDetail(event);
      }
    });
  });
};

const showAuditDetail = (event) => {
  if (!auditModal || !auditModalContent) return;
  
  const outcomeClass = event.outcome === 'success'
    ? 'pill success'
    : event.outcome === 'denied'
    ? 'pill denied'
    : 'pill failure';
  
  const isUser = event.actor?.kind === 'user';
  const actorInitial = isUser 
    ? (event.actor.user_id || 'U').charAt(0).toUpperCase()
    : 'S';
  const actorName = isUser ? escapeHtml(event.actor.user_id || 'Unbekannt') : 'System';
  const actorRole = isUser ? escapeHtml(event.actor.role || '–') : 'Automatisch';
  
  const actorHtml = `
    <div class="audit-detail-actor">
      <div class="actor-icon">${actorInitial}</div>
      <div class="actor-info">
        <span class="actor-name">${actorName}</span>
        <span class="actor-role">${actorRole}</span>
      </div>
    </div>
  `;
  
  const metadataHtml = (event.metadata && event.metadata.length > 0)
    ? `<div class="audit-metadata-list">
        ${event.metadata.map((m) => `
          <div class="audit-metadata-item">
            <span class="audit-metadata-key">${escapeHtml(m.key)}</span>
            <span class="audit-metadata-val${m.redacted ? ' redacted' : ''}">${m.redacted ? '••••••••' : escapeHtml(m.value || '–')}</span>
          </div>
        `).join('')}
      </div>`
    : '<div style="color: var(--text-muted); font-size: 0.85rem; padding: 10px 14px; background: var(--bg-elevated); border-radius: var(--radius-md); border: 1px solid var(--border-subtle);">Keine Metadaten vorhanden</div>';
  
  auditModalContent.innerHTML = `
    <div class="audit-detail-grid">
      <div class="audit-detail-row">
        <span class="audit-detail-label">Zeitpunkt</span>
        <span class="audit-detail-value">${formatTimestamp(event.timestamp)}</span>
      </div>
      <div class="audit-detail-row">
        <span class="audit-detail-label">Aktion</span>
        <span class="audit-detail-value mono">${escapeHtml(event.action || '–')}</span>
      </div>
      <div class="audit-detail-row">
        <span class="audit-detail-label">Target</span>
        <span class="audit-detail-value mono">${escapeHtml(event.target || '–')}</span>
      </div>
      <div class="audit-detail-row">
        <span class="audit-detail-label">Ergebnis</span>
        <span class="audit-detail-value"><span class="${outcomeClass}">${escapeHtml(event.outcome || '–')}</span></span>
      </div>
      <div class="audit-detail-row">
        <span class="audit-detail-label">Akteur</span>
        ${actorHtml}
      </div>
      <div class="audit-detail-row">
        <span class="audit-detail-label">Metadaten</span>
        ${metadataHtml}
      </div>
    </div>
  `;
  
  auditModal.dataset.visible = 'true';
};

const closeAuditModal = () => {
  if (auditModal) {
    auditModal.dataset.visible = 'false';
  }
};

// Service Detail Modal
const showServiceDetail = (svc) => {
  if (!serviceModal || !serviceModalContent) return;
  
  const status = svc.status ?? 'unknown';
  const diagnostics = svc.diagnostics || {};
  const heartbeat = formatHeartbeatMetric(diagnostics.last_heartbeat_seconds);
  const latencyP50 = formatLatencyMetric(diagnostics.latency_p50_ms);
  const latencyP95 = formatLatencyMetric(diagnostics.latency_p95_ms);
  const errorRate = formatErrorRateMetric(diagnostics.error_rate_pct);
  const health = (diagnostics.state || status).toLowerCase();
  const tags = (svc.tags ?? []).join(', ') || '–';
  const note = svc.note ?? '–';
  
  serviceModalContent.innerHTML = `
    <div class="service-detail-grid">
      <div class="service-detail-header">
        <h4>${svc.name ?? svc.id}</h4>
        <span class="service-detail-id">${svc.id}</span>
      </div>
      
      <div class="service-detail-section">
        <h5>Status</h5>
        <div class="service-detail-row">
          <span class="service-detail-label">Status</span>
          <span class="${statusClass(status)}">${status}</span>
        </div>
        <div class="service-detail-row">
          <span class="service-detail-label">Health</span>
          <span class="${healthClass(health)}">${health}</span>
        </div>
        <div class="service-detail-row">
          <span class="service-detail-label">Letzter Heartbeat</span>
          <span class="service-detail-value mono">${heartbeat}</span>
        </div>
      </div>
      
      <div class="service-detail-section">
        <h5>Performance</h5>
        <div class="service-metrics-grid">
          <div class="metric-box">
            <span class="metric-label">P50 Latenz</span>
            <span class="metric-value">${latencyP50}</span>
          </div>
          <div class="metric-box">
            <span class="metric-label">P95 Latenz</span>
            <span class="metric-value">${latencyP95}</span>
          </div>
          <div class="metric-box">
            <span class="metric-label">Fehlerrate</span>
            <span class="metric-value ${parseFloat(errorRate) > 1 ? 'error' : ''}">${errorRate}</span>
          </div>
        </div>
      </div>
      
      <div class="service-detail-section">
        <h5>Info</h5>
        <div class="service-detail-row">
          <span class="service-detail-label">Tags</span>
          <span class="service-detail-value">${tags}</span>
        </div>
        <div class="service-detail-row">
          <span class="service-detail-label">Hinweis</span>
          <span class="service-detail-value">${note}</span>
        </div>
      </div>
    </div>
  `;
  
  serviceModal.dataset.visible = 'true';
};

const closeServiceModal = () => {
  if (serviceModal) {
    serviceModal.dataset.visible = 'false';
  }
};

// Modal close handlers
if (auditModalClose) {
  auditModalClose.addEventListener('click', closeAuditModal);
}
if (serviceModalClose) {
  serviceModalClose.addEventListener('click', closeServiceModal);
}
if (auditModal) {
  auditModal.addEventListener('click', (e) => {
    if (e.target === auditModal) {
      closeAuditModal();
    }
  });
}
if (serviceModal) {
  serviceModal.addEventListener('click', (e) => {
    if (e.target === serviceModal) {
      closeServiceModal();
    }
  });
}
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    if (auditModal?.dataset.visible === 'true') {
      closeAuditModal();
    }
    if (serviceModal?.dataset.visible === 'true') {
      closeServiceModal();
    }
  }
});

const renderAuditPreview = () => {
  if (!auditPreviewList) {
    return;
  }
  if (!auditCache || auditCache.length === 0) {
    if (lastAuditPreviewHtml !== '') {
      auditPreviewList.innerHTML = '';
      lastAuditPreviewHtml = '';
    }
    if (auditPreviewEmpty && auditPreviewEmpty.dataset.visible !== 'true') {
      auditPreviewEmpty.dataset.visible = 'true';
    }
    return;
  }
  const items = auditCache.slice(0, 5);
  const newHtml = items
    .map((event) => {
      const relative = formatRelativeTime(Date.parse(event.timestamp ?? '')) || '–';
      const outcomeRaw = (event.outcome ?? 'unknown').toLowerCase();
      const outcomeText = escapeHtml(event.outcome ?? 'unknown');
      return `
        <li class="audit-preview-item">
          <span class="preview-time">${escapeHtml(relative)}</span>
          <div class="preview-body">
            <strong>${escapeHtml(event.action ?? 'unbekannt')}</strong>
            <span>${escapeHtml(event.target ?? '—')}</span>
          </div>
          <span class="preview-outcome preview-${escapeHtml(outcomeRaw)}">${outcomeText}</span>
        </li>
      `;
    })
    .join('');
  
  // Only update DOM if content changed
  if (newHtml !== lastAuditPreviewHtml) {
    auditPreviewList.innerHTML = newHtml;
    lastAuditPreviewHtml = newHtml;
  }
  if (auditPreviewEmpty && auditPreviewEmpty.dataset.visible !== 'false') {
    auditPreviewEmpty.dataset.visible = 'false';
  }
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

/* ============================================
   SERVER-SENT EVENTS (SSE)
   ============================================ */

/**
 * Connect to the event stream for real-time updates
 */
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

const healthClass = (state) => {
  switch (state) {
    case 'healthy':
      return 'status-pill status-active';
    case 'degraded':
      return 'status-pill status-degraded';
    case 'stale':
      return 'status-pill status-standby';
    default:
      return 'status-pill status-unknown';
  }
};

const formatHeartbeatMetric = (seconds) => {
  if (typeof seconds !== 'number' || Number.isNaN(seconds)) {
    return '–';
  }
  if (seconds < 1) {
    return '<1s';
  }
  if (seconds < 60) {
    return `${Math.round(seconds)}s`;
  }
  const minutes = seconds / 60;
  if (minutes < 60) {
    return `${Math.round(minutes)}m`;
  }
  const hours = minutes / 60;
  if (hours < 24) {
    return `${Math.round(hours)}h`;
  }
  const days = hours / 24;
  return `${Math.round(days)}d`;
};

const formatLatencyMetric = (value) => {
  if (typeof value === 'number' && Number.isFinite(value)) {
    return value.toFixed(0);
  }
  return '–';
};

const formatErrorRateMetric = (value) => {
  if (typeof value === 'number' && Number.isFinite(value)) {
    return value.toFixed(1);
  }
  return '–';
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
  const baseUrl = info.http?.base_url ?? `http://${host}:${port}`;
  metaEl.innerHTML = `
    <div class="info-chip">
      <span class="chip-icon">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="12" r="10"/>
          <circle cx="12" cy="12" r="3"/>
        </svg>
      </span>
      <div class="chip-content">
        <span class="chip-label">Endpoint</span>
        <span class="chip-value">${escapeHtml(`${host}:${port}`)}</span>
      </div>
    </div>
    <div class="info-chip http-chip">
      <span class="chip-icon">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/>
          <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>
        </svg>
      </span>
      <div class="chip-content">
        <span class="chip-label">HTTP</span>
        <span class="chip-value">${escapeHtml(baseUrl)}</span>
      </div>
    </div>
  `;
};

function updateServiceFilterClearState() {
  if (!serviceFilterClear) {
    return;
  }
  serviceFilterClear.dataset.visible = serviceFilterValue ? 'true' : 'false';
}

function setServiceFilterValue(value) {
  serviceFilterValue = value.trim().toLowerCase();
  updateServiceFilterClearState();
  renderServicesTable();
}

function setServiceStatusFilter(value) {
  serviceStatusFilter = (value || 'all').toLowerCase();
  serviceStatusButtons.forEach((button) => {
    const active = (button.dataset.serviceFilterStatus || 'all').toLowerCase() === serviceStatusFilter;
    button.dataset.active = active ? 'true' : 'false';
  });
  renderServicesTable();
}

const normalizeStatus = (value) => (value || '').toLowerCase();

const serviceMatchesStatus = (svc) => {
  if (serviceStatusFilter === 'all') {
    return true;
  }
  return normalizeStatus(svc.status) === serviceStatusFilter;
};

const serviceMatchesFilter = (svc) => {
  if (!serviceFilterValue) {
    return true;
  }
  const parts = [
    svc.id,
    svc.name,
    svc.note,
    Array.isArray(svc.tags) ? svc.tags.join(' ') : '',
  ]
    .filter(Boolean)
    .map((entry) => entry.toLowerCase());
  return parts.some((part) => part.includes(serviceFilterValue));
};

/* ============================================
   SERVICES TABLE RENDERING
   ============================================ */

/**
 * Render the services table with filtering and sorting
 */
function renderServicesTable() {
  if (servicesCache.size === 0) {
    const emptyHtml = `<tr><td colspan="6">${serviceTableEmptyMessage}</td></tr>`;
    if (lastServicesTableHtml !== emptyHtml) {
      svcBody.innerHTML = emptyHtml;
      lastServicesTableHtml = emptyHtml;
    }
    if (metaServices) {
      if (serviceMetaOverride) {
        metaServices.textContent = serviceMetaOverride;
      } else {
        metaServices.textContent = '0 Services';
      }
    }
    renderServiceSummary({});
    renderServiceIncidents();
    return;
  }
  const items = Array.from(servicesCache.values()).sort((a, b) =>
    (a.id || '').localeCompare(b.id || ''),
  );
  const filtered = items.filter((svc) => serviceMatchesStatus(svc) && serviceMatchesFilter(svc));
  const total = items.length;
  const visible = filtered.length;
  if (metaServices) {
    if (serviceMetaOverride) {
      metaServices.textContent = serviceMetaOverride;
    } else if (visible === total) {
      metaServices.textContent = `${total} Services`;
    } else {
      metaServices.textContent = `${visible}/${total} Services gefiltert`;
    }
  }
  if (filtered.length === 0) {
    const noFilterHtml = '<tr><td colspan="6">Keine Services passend zum Filter.</td></tr>';
    if (lastServicesTableHtml !== noFilterHtml) {
      svcBody.innerHTML = noFilterHtml;
      lastServicesTableHtml = noFilterHtml;
    }
    renderServiceIncidents();
    return;
  }
  const newHtml = filtered
    .map((svc) => {
      const status = svc.status ?? 'unknown';
      const note = svc.note ?? '–';
      const tags = renderTags(svc.tags ?? []);
      return `
        <tr class="service-row clickable" data-service-id="${svc.id}">
          <td class="service-id">${svc.id}</td>
          <td class="service-name">${svc.name ?? svc.id}</td>
          <td class="status-cell"><span class="${statusClass(status)}" title="${status}">${status}</span></td>
          <td class="tags-cell"><div class="tag-list">${tags}</div></td>
          <td class="note-cell">${note}</td>
          <td class="actions-cell">${renderRowActions(svc)}</td>
        </tr>
      `;
    })
    .join('');
  
  // Only update DOM if content actually changed
  if (newHtml !== lastServicesTableHtml) {
    svcBody.innerHTML = newHtml;
    lastServicesTableHtml = newHtml;
    attachServiceRowHandlers();
  }
  renderServiceIncidents();
}

function attachServiceRowHandlers() {
  const rows = svcBody.querySelectorAll('.service-row[data-service-id]');
  rows.forEach((row) => {
    row.addEventListener('click', (e) => {
      // Don't open modal if clicking on action buttons
      if (e.target.closest('.action-btn') || e.target.closest('button')) return;
      const serviceId = row.dataset.serviceId;
      const svc = servicesCache.get(serviceId);
      if (svc) {
        showServiceDetail(svc);
      }
    });
  });
}

// Cache for detecting actual data changes
let lastServicesJson = '';

const updateServices = (payload) => {
  // Check if data actually changed
  const newJson = JSON.stringify(payload?.services || []);
  if (newJson === lastServicesJson) {
    // Data unchanged, skip re-render
    return;
  }
  lastServicesJson = newJson;
  
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
  serviceTableEmptyMessage = 'Keine Services registriert.';
  serviceMetaOverride = null;
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

/* ============================================
   API FUNCTIONS
   ============================================ */

/**
 * Fetch with authentication token
 * @param {string} url - API endpoint
 * @param {Object} [options] - Fetch options
 * @returns {Promise<Response>}
 */
const fetchWithToken = (url, options = {}) => {
  const headers = { ...options.headers, ...authHeaders() };
  return fetch(url, { ...options, headers });
};

/**
 * Perform action on a single service
 * @param {string} id - Service ID
 * @param {string} action - Action type (start, stop, restart)
 * @param {boolean} [force=false] - Force destructive action
 * @returns {Promise<Object>}
 */
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

/* ============================================
   DATA LOADING
   ============================================ */

/**
 * Load all dashboard data from API
 * @param {Object} options
 * @param {boolean} [options.background=false] - If true, suppress error alerts
 */
const loadAll = async ({ background = false } = {}) => {
  if (isLoading) {
    return;
  }
  isLoading = true;
  if (!background) {
    showAlert('');
  }
  try {
    const [infoRes, servicesRes, metricsRes, modulesRes] = await Promise.all([
      fetchWithToken('/info'),
      fetchWithToken('/services'),
      fetchWithToken('/metrics'),
      fetchWithToken('/modules/installed'),
    ]);

    if (infoRes.ok) {
      updateMeta(await infoRes.json());
    }

    if (servicesRes.ok) {
      updateServices(await servicesRes.json());
    } else {
      servicesCache = new Map();
      serviceTableEmptyMessage = 'Fehler beim Laden der Services.';
      serviceMetaOverride = `Fehler (${servicesRes.status})`;
      renderServicesTable();
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

    if (modulesRes.ok) {
      updateModulesBoard(await modulesRes.json());
    } else {
      setModulesError(`Module nicht verfügbar (${modulesRes.status})`);
    }

    if (!auditHistoryLoaded && !auditHistoryLoading) {
      fetchAuditHistory(currentAuditRange, { background: background || !document.hasFocus() }).catch(() => {});
    }
    lastRefreshAt = Date.now();
    updateRefreshNote();
  } catch (error) {
    console.error('[Fenrir] loadAll failed:', error);
    servicesCache = new Map();
    serviceTableEmptyMessage = 'Netzwerkfehler: Daten konnten nicht geladen werden.';
    serviceMetaOverride = 'Fehler';
    renderServicesTable();
    showAlert(background ? 'Auto-Refresh fehlgeschlagen – Verbindung prüfen.' : `Netzwerkfehler: ${error.message || 'Bitte Verbindung prüfen.'}`);
    auditBody.innerHTML = '<tr><td colspan="4">Netzwerkfehler – keine Audit-Daten.</td></tr>';
    auditMeta.textContent = 'Fehler';
    auditHistoryLoaded = false;
    setUptimeBase(null);
    healthValue.textContent = 'unbekannt';
    healthNote.textContent = 'Telemetrie nicht verfügbar.';
    markTelemetryUnavailable('Telemetrie nicht verfügbar.');
    resetServiceMetrics('Telemetrie nicht verfügbar.');
    setModulesError('Modulübersicht nicht verfügbar.');
  } finally {
    isLoading = false;
  }
};

const scheduleRefresh = () => {
  if (refreshHandle !== null) {
    window.clearInterval(refreshHandle);
  }
  refreshHandle = window.setInterval(() => {
    // Skip refresh if tab is hidden
    if (document.hidden || !isPageVisible) {
      return;
    }
    // Smart refresh based on visible page
    loadAll({ background: true });
  }, REFRESH_INTERVAL_MS);
};

const persistToken = () => {
  const token = currentToken();
  saveToken(token);
  setTokenUi(token);
};

/* ============================================
   EVENT HANDLERS
   ============================================ */

// Token Input Events
tokenInput.addEventListener('input', () => {
  persistToken();
});

tokenInput.addEventListener('change', () => {
  persistToken();
  connectEventStream();
});

tokenInput.addEventListener('keydown', (event) => {
  if (event.key === 'Enter') {
    event.preventDefault();
    persistToken();
    connectEventStream();
    tokenInput.blur();
  }
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

/* ============================================
   APPLICATION INITIALIZATION
   ============================================ */

/**
 * Initialize the dashboard application
 * - Load token from storage
 * - Setup event stream connection
 * - Load initial data
 * - Schedule auto-refresh
 */
const init = async () => {
  const token = loadToken();
  tokenInput.value = token;
  setTokenUi(token);
  renderSeriesToggles();
  connectEventStream();
  await loadAll();
  scheduleRefresh();
  // Note: visibility change handler already set up in performance utilities
  // Focus handler with throttle to prevent rapid refreshes
  const throttledRefresh = throttle(() => loadAll({ background: true }), 5000);
  window.addEventListener('focus', throttledRefresh);
  
  // Re-render chart on resize for proper DPI scaling
  let resizeTimeout;
  window.addEventListener('resize', () => {
    clearTimeout(resizeTimeout);
    resizeTimeout = setTimeout(() => {
      if (metricHistory.length > 0) {
        renderMetricChart();
      }
    }, 150);
  });
};

/* ============================================
   BOOTSTRAP
   ============================================ */

// Start the application
init();
