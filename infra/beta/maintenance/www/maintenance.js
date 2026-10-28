/*
 * The Athenos maintenance page: picks the texts (German or English, from the
 * browser), shows the reason Caddy's status file names, and polls every 5 s:
 *
 *   /_maintenance/status.json   Caddy, from /var/lib/athenos-maintenance
 *                               ({"active":false} when switched off)
 *   <API>/health                the app itself, through Caddy
 *
 * Once maintenance is off and the app answers healthy, the page loads the
 * address it was opened with, which is the app again. A note in
 * sessionStorage tells the app that it comes back from maintenance (its
 * "Welcome back" toast).
 *
 * `#preview=update|fix|wartung|unplanned` (plus `&eta=HH:MM`,
 * `&message=…`) renders a variant without polling or reloading.
 *
 * Plain ES2017, no dependencies, nothing that can throw past a try.
 */
(function () {
  'use strict';

  var STATUS_URL = '/_maintenance/status.json';
  var HEALTH_URL = '/gateway/services/module%3Aathenos-api%3A%3Aapi-gateway/health';
  var POLL_MS = 5000;
  var RETURN_KEY = 'athenos.maintenance.returning';
  var REASONS = ['update', 'fix', 'wartung', 'unplanned'];

  var TEXTS = {
    de: {
      update: {
        title: 'Athenos bekommt ein Update',
        emoji: '✨',
        lead: 'Wir spielen gerade eine neue Version ein. In ein paar Minuten bist du wieder dabei – mit neuen Funktionen und Verbesserungen.',
      },
      fix: {
        title: 'Kurze Pause für Feinschliff',
        emoji: '🔧',
        lead: 'Wir beheben gerade einen Fehler, damit alles wieder rund läuft. Das dauert nur einen Moment.',
      },
      wartung: {
        title: 'Wir pflegen gerade die Technik',
        emoji: '🛠️',
        lead: 'Athenos ist für eine geplante Wartung kurz offline. Deine Daten sind sicher.',
      },
      unplanned: {
        title: 'Athenos ist gleich wieder da',
        emoji: '',
        lead: 'Gerade gibt es ein kurzes technisches Problem – wir sind schon dran. Deine Daten sind sicher.',
      },
      eta: 'Voraussichtlich zurück um {time}.',
      etaDay: 'Voraussichtlich zurück am {day} um {time}.',
      overdue: 'Es dauert etwas länger als geplant – gleich geschafft.',
      auto: 'Diese Seite lädt automatisch neu.',
      back: 'Athenos ist wieder da – einen Moment …',
      questions: 'Fragen?',
      impressum: 'Impressum',
      doc: 'Athenos · Gleich wieder da',
    },
    en: {
      update: {
        title: 'Athenos is getting an update',
        emoji: '✨',
        lead: "We're rolling out a new version. You'll be back in a few minutes – with new features and improvements.",
      },
      fix: {
        title: 'A quick pause for some fine-tuning',
        emoji: '🔧',
        lead: "We're fixing an issue so everything runs smoothly again. This only takes a moment.",
      },
      wartung: {
        title: "We're looking after the engine room",
        emoji: '🛠️',
        lead: 'Athenos is briefly offline for planned maintenance. Your data is safe.',
      },
      unplanned: {
        title: 'Athenos will be right back',
        emoji: '',
        lead: "We're having a brief technical hiccup and are already on it. Your data is safe.",
      },
      eta: 'Expected back at {time}.',
      etaDay: 'Expected back on {day} at {time}.',
      overdue: "It's taking a little longer than planned – almost there.",
      auto: 'This page reloads automatically.',
      back: 'Athenos is back – one moment …',
      questions: 'Questions?',
      impressum: 'Legal notice',
      doc: 'Athenos · Back in a moment',
    },
  };

  function language() {
    var list = [];
    try {
      list = navigator.languages && navigator.languages.length ? navigator.languages : [navigator.language];
    } catch (e) {
      list = [];
    }
    for (var i = 0; i < list.length; i++) {
      var tag = String(list[i] || '').toLowerCase();
      if (tag.indexOf('de') === 0) return 'de';
      if (tag.indexOf('en') === 0) return 'en';
    }
    return 'de';
  }

  var lang = language();
  var t = TEXTS[lang];
  var root = document.documentElement;
  var timer = null;
  var leaving = false;

  function el(selector) {
    return document.querySelector(selector);
  }

  function setText(key, value) {
    var node = el('[data-text="' + key + '"]');
    if (node) node.textContent = value;
  }

  function two(n) {
    return (n < 10 ? '0' : '') + n;
  }

  function formatTime(date) {
    try {
      return new Intl.DateTimeFormat(lang === 'de' ? 'de-CH' : 'en-GB', {
        hour: '2-digit',
        minute: '2-digit',
      }).format(date);
    } catch (e) {
      return two(date.getHours()) + ':' + two(date.getMinutes());
    }
  }

  function formatDay(date) {
    try {
      return new Intl.DateTimeFormat(lang === 'de' ? 'de-CH' : 'en-GB', {
        weekday: 'long',
        day: 'numeric',
        month: 'long',
      }).format(date);
    } catch (e) {
      return date.toDateString();
    }
  }

  /** The ETA line: "… um <strong>20:15</strong>.", built without innerHTML. */
  function renderEta(until) {
    var row = el('[data-slot="eta"]');
    var text = el('[data-slot="eta-text"]');
    if (!row || !text) return;
    var date = until ? new Date(until) : null;
    if (!date || isNaN(date.getTime())) {
      row.hidden = true;
      return;
    }
    row.hidden = false;
    while (text.firstChild) text.removeChild(text.firstChild);
    if (date.getTime() <= Date.now()) {
      text.textContent = t.overdue;
      return;
    }
    var sameDay = new Date().toDateString() === date.toDateString();
    var template = sameDay ? t.eta : t.etaDay;
    var parts = template.split(/(\{time\}|\{day\})/);
    for (var i = 0; i < parts.length; i++) {
      if (parts[i] === '{time}') {
        var strong = document.createElement('strong');
        strong.textContent = formatTime(date);
        text.appendChild(strong);
      } else if (parts[i] === '{day}') {
        text.appendChild(document.createTextNode(formatDay(date)));
      } else if (parts[i]) {
        text.appendChild(document.createTextNode(parts[i]));
      }
    }
  }

  function render(state) {
    var reason = REASONS.indexOf(state.reason) >= 0 ? state.reason : 'unplanned';
    var copy = t[reason];
    root.setAttribute('data-variant', reason);
    root.setAttribute('lang', lang);
    setText('title', copy.title);
    setText('emoji', copy.emoji);
    setText('lead', copy.lead);
    setText('auto', leaving ? t.back : t.auto);
    setText('questions', t.questions);
    setText('impressum', t.impressum);
    document.title = 'Athenos · ' + copy.title;
    var note = el('[data-slot="message"]');
    if (note) {
      var message = typeof state.message === 'string' ? state.message.trim() : '';
      note.textContent = message;
      note.hidden = !message;
    }
    renderEta(state.until || null);
  }

  function getJson(url) {
    var sep = url.indexOf('?') >= 0 ? '&' : '?';
    return fetch(url + sep + '_=' + Date.now(), {
      cache: 'no-store',
      credentials: 'same-origin',
      headers: { Accept: 'application/json' },
      redirect: 'error',
    }).then(function (response) {
      // Caddy marks its own answers: a 200 with that header is still us.
      var ours = response.headers.get('x-athenos-maintenance');
      return response
        .json()
        .catch(function () {
          return null;
        })
        .then(function (body) {
          return { ok: response.ok && !ours, status: response.status, body: body };
        });
    });
  }

  function appIsBack() {
    return getJson(HEALTH_URL).then(function (res) {
      return res.ok && res.body && res.body.status === 'healthy';
    });
  }

  function returnToApp() {
    leaving = true;
    setText('auto', t.back);
    try {
      window.sessionStorage.setItem(RETURN_KEY, String(Date.now()));
    } catch (e) {
      // Without storage the app simply shows no welcome-back toast.
    }
    var path = window.location.pathname;
    // Opened as /_maintenance/… itself: back to the app's start page.
    var target = path.indexOf('/_maintenance') === 0 ? '/' : path + window.location.search;
    window.setTimeout(function () {
      window.location.replace(target);
    }, 600);
  }

  function schedule(delay) {
    window.clearTimeout(timer);
    if (leaving || document.hidden) return;
    timer = window.setTimeout(tick, delay);
  }

  function tick(done) {
    root.setAttribute('data-state', 'checking');
    getJson(STATUS_URL)
      .then(function (res) {
        var state = res.body && typeof res.body === 'object' ? res.body : { active: false };
        if (state.active === true) {
          render(state);
          return false;
        }
        render({ reason: 'unplanned' });
        return appIsBack();
      })
      .catch(function () {
        return false;
      })
      .then(function (back) {
        root.removeAttribute('data-state');
        if (typeof done === 'function') done();
        if (back) returnToApp();
        else schedule(POLL_MS);
      });
  }

  function preview() {
    var hash = window.location.hash.replace(/^#/, '');
    if (hash.indexOf('preview=') < 0) return null;
    var params = {};
    hash.split('&').forEach(function (pair) {
      var i = pair.indexOf('=');
      if (i > 0) params[pair.slice(0, i)] = decodeURIComponent(pair.slice(i + 1).replace(/\+/g, ' '));
    });
    var until = null;
    if (/^\d{2}:\d{2}$/.test(params.eta || '')) {
      var d = new Date();
      d.setHours(Number(params.eta.slice(0, 2)), Number(params.eta.slice(3, 5)), 0, 0);
      if (d.getTime() <= Date.now()) d.setDate(d.getDate() + 1);
      until = d.toISOString();
    }
    if (params.lang === 'de' || params.lang === 'en') {
      lang = params.lang;
      t = TEXTS[lang];
    }
    return { active: true, reason: params.preview, until: until, message: params.message || '' };
  }

  function start() {
    var shown = preview();
    if (shown) {
      render(shown);
      return;
    }
    // The card stays invisible until the first status names the reason
    // (a local request, milliseconds), so no variant flashes past.
    root.setAttribute('data-boot', '');
    var reveal = function () {
      root.removeAttribute('data-boot');
    };
    window.setTimeout(reveal, 1500);
    render({ reason: root.getAttribute('data-variant') || 'unplanned' });
    tick(reveal);
    document.addEventListener('visibilitychange', function () {
      if (!document.hidden) schedule(0);
    });
    window.addEventListener('online', function () {
      schedule(0);
    });
  }

  // The Impressum page shares this file only for the language switch.
  if (root.getAttribute('data-page') === 'impressum') {
    var nodes = document.querySelectorAll('[data-lang]');
    for (var i = 0; i < nodes.length; i++) nodes[i].hidden = nodes[i].getAttribute('data-lang') !== lang;
    root.setAttribute('lang', lang);
    return;
  }
  start();
})();
