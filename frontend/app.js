import { mapStyle, EMPTY_FC } from '/map-style.js';

const $ = (id) => document.getElementById(id);
const log = $('log'), input = $('input'), send = $('send');
const statusEl = $('status'), itinEl = $('itinerary'), legsEl = $('legs');

/* ------------------------------------------------------------------- map */

const style = structuredClone(mapStyle);
style.sources.route = { type: 'geojson', data: EMPTY_FC };
style.sources['route-active'] = { type: 'geojson', data: EMPTY_FC };
style.sources.pois = { type: 'geojson', data: EMPTY_FC };

// Default view: California. NOTE — the basemap tileset currently covers Indonesia
// only, so this renders as empty ground until the North America build lands. The
// view is deliberately set ahead of the data so the default does not need changing
// again when it does.
const DEFAULT_VIEW = { center: [-119.42, 36.78], zoom: 5.4 };

const map = new maplibregl.Map({
  container: 'map',
  style,
  center: DEFAULT_VIEW.center,
  zoom: DEFAULT_VIEW.zoom,
  attributionControl: { compact: true },
  dragRotate: false,
});
map.addControl(new maplibregl.NavigationControl({ showCompass: false }), 'bottom-right');

const routes = [];   // one entry per `route` tool result — i.e. one leg
let pois = [];
let mapReady = false;
map.on('load', () => { mapReady = true; });

const setSrc = (id, data) => {
  if (mapReady) map.getSource(id)?.setData(data);
};

map.on('move', () => {
  const c = map.getCenter();
  const fmt = (v, pos, neg) =>
    `${Math.abs(v).toFixed(0)}°${String(Math.round((Math.abs(v) % 1) * 60)).padStart(2, '0')}′${v >= 0 ? pos : neg}`;
  $('readout').textContent = `${fmt(c.lat, 'N', 'S')} ${fmt(c.lng, 'E', 'W')} · Z${map.getZoom().toFixed(1)}`;
});

function boundsOf(features) {
  const b = new maplibregl.LngLatBounds();
  let any = false;
  const walk = (coords) => {
    if (typeof coords[0] === 'number') { b.extend([coords[0], coords[1]]); any = true; }
    else coords.forEach(walk);
  };
  for (const f of features) if (f?.geometry?.coordinates) walk(f.geometry.coordinates);
  return any ? b : null;
}

function fitTo(features, padding = 90) {
  const b = boundsOf(features);
  if (b) map.fitBounds(b, { padding, duration: 1100, maxZoom: 13 });
}

/* ------------------------------------------------------------ transcript */

function el(cls, text) {
  const d = document.createElement('div');
  d.className = cls;
  if (text) d.textContent = text;
  return d;
}

const atBottom = () => log.scrollHeight - log.scrollTop - log.clientHeight < 120;
function scroll(force) {
  if (force || atBottom()) log.scrollTop = log.scrollHeight;
}

function clearSeed() {
  log.querySelector('.seed')?.remove();
}

/** Compact one-line summary of a tool call, in the instrument register. */
function toolArg(name, input) {
  try {
    if (name === 'geocode') return input.query ?? '';
    if (name === 'search_poi') return (input.tags || []).join(' · ');
    if (name === 'route' || name === 'elevation_profile') {
      const n = (input.waypoints || []).length;
      const cm = input.custom_model ? ' +model' : '';
      return `${n} waypoints${cm}`;
    }
  } catch { /* fall through */ }
  return '';
}

/* --------------------------------------------------------------- streaming */

let history = [];
let busy = false;

async function ask(question) {
  if (busy) return;
  busy = true;
  send.disabled = true;
  clearSeed();

  log.appendChild(el('msg user', question));
  scroll(true);

  history.push({ role: 'user', content: question });

  statusEl.textContent = 'PLANNING…';
  statusEl.classList.add('busy');

  let agentEl = null;    // current assistant text block
  let thinkEl = null;    // current reasoning block
  let answer = '';
  const pending = new Map();  // tool name -> its live row

  try {
    const res = await fetch('/api/chat', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ messages: history }),
    });

    if (!res.ok) {
      const detail = await res.text();
      throw new Error(detail || `server returned ${res.status}`);
    }

    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buf = '';

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });

      // SSE frames are blank-line separated and may straddle chunk boundaries.
      let sep;
      while ((sep = buf.indexOf('\n\n')) !== -1) {
        const frame = buf.slice(0, sep);
        buf = buf.slice(sep + 2);

        const line = frame.split('\n').find((l) => l.startsWith('data:'));
        if (!line) continue;
        let ev;
        try { ev = JSON.parse(line.slice(5).trim()); } catch { continue; }

        switch (ev.type) {
          case 'thinking': {
            if (!thinkEl) { thinkEl = el('think'); log.appendChild(thinkEl); }
            thinkEl.textContent += ev.text;
            scroll();
            break;
          }
          case 'text': {
            // Reasoning is finished once prose starts — collapse it away.
            thinkEl = null;
            if (!agentEl) { agentEl = el('msg agent'); log.appendChild(agentEl); }
            answer += ev.text;
            agentEl.textContent += ev.text;
            scroll();
            break;
          }
          case 'tool_start': {
            agentEl = null; thinkEl = null;
            const row = el('tool');
            row.innerHTML =
              `<span class="dot"></span><span class="name"></span><span class="arg"></span>`;
            row.querySelector('.name').textContent = ev.name;
            row.querySelector('.arg').textContent = toolArg(ev.name, ev.input || {});
            log.appendChild(row);
            // Several calls of the same tool can be in flight at once; keep a
            // queue per name so tool_end retires them in order.
            if (!pending.has(ev.name)) pending.set(ev.name, []);
            pending.get(ev.name).push(row);
            scroll();
            break;
          }
          case 'tool_end': {
            const row = pending.get(ev.name)?.shift();
            if (row) row.classList.add(ev.ok ? 'done' : 'failed');
            break;
          }
          case 'geometry': {
            routes.push(ev.geojson);
            setSrc('route', { type: 'FeatureCollection', features: routes });
            renderItinerary();
            fitTo(routes);
            break;
          }
          case 'markers': {
            const incoming = ev.geojson?.features || [];
            // Dedupe by rounded position so repeated searches don't stack markers.
            const seen = new Set(pois.map((f) => f.geometry.coordinates.map((n) => n.toFixed(4)).join()));
            for (const f of incoming) {
              const k = f.geometry.coordinates.map((n) => n.toFixed(4)).join();
              if (!seen.has(k)) { seen.add(k); pois.push(f); }
            }
            setSrc('pois', { type: 'FeatureCollection', features: pois });
            break;
          }
          case 'refused': {
            log.appendChild(el('msg error', ev.message));
            scroll(true);
            break;
          }
          case 'error': {
            log.appendChild(el('msg error', ev.message));
            scroll(true);
            break;
          }
          case 'done':
            break;
        }
      }
    }

    if (answer.trim()) history.push({ role: 'assistant', content: answer });
  } catch (e) {
    log.appendChild(el('msg error', String(e.message || e)));
    scroll(true);
  } finally {
    busy = false;
    send.disabled = false;
    statusEl.textContent = '';
    statusEl.classList.remove('busy');
    input.focus();
  }
}

/* --------------------------------------------------------------- itinerary */

const km = (m) => `${(m / 1000).toFixed(0)} km`;
const hm = (ms) => {
  const t = Math.round(ms / 60000);
  const h = Math.floor(t / 60), m = t % 60;
  return h ? `${h}h${String(m).padStart(2, '0')}` : `${m}m`;
};

function renderItinerary() {
  if (!routes.length) { itinEl.hidden = true; return; }
  itinEl.hidden = false;
  legsEl.innerHTML = '';

  routes.forEach((f, i) => {
    const p = f.properties || {};
    const btn = document.createElement('button');
    btn.className = 'leg';
    btn.type = 'button';
    btn.innerHTML =
      `<span class="leg-idx">${String(i + 1).padStart(2, '0')}</span>
       <span class="leg-body">
         <span class="leg-name">Leg ${i + 1}</span>
         <span class="leg-stat"></span>
       </span>`;
    btn.querySelector('.leg-name').textContent = `Leg ${i + 1}`;
    btn.querySelector('.leg-stat').textContent =
      `${km(p.distance_m || 0)} · ${hm(p.duration_ms || 0)}`;

    btn.addEventListener('click', () => {
      const already = btn.classList.contains('active');
      legsEl.querySelectorAll('.leg').forEach((b) => b.classList.remove('active'));
      if (already) {
        setSrc('route-active', EMPTY_FC);
        fitTo(routes);
      } else {
        btn.classList.add('active');
        setSrc('route-active', { type: 'FeatureCollection', features: [f] });
        fitTo([f], 140);
      }
    });

    legsEl.appendChild(btn);
  });
}

/* ------------------------------------------------------------------ input */

$('composer').addEventListener('submit', (e) => {
  e.preventDefault();
  const q = input.value.trim();
  if (!q) return;
  input.value = '';
  input.style.height = 'auto';
  ask(q);
});

input.addEventListener('input', () => {
  input.style.height = 'auto';
  input.style.height = `${Math.min(input.scrollHeight, 128)}px`;
});

input.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    $('composer').requestSubmit();
  }
});

document.querySelectorAll('.seed-btn').forEach((b) => {
  b.addEventListener('click', () => ask(b.dataset.q));
});

input.focus();
