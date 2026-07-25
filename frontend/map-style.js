// Hand-tuned dark cartographic style over the Planetiler/OpenMapTiles tileset.
//
// Deviation from design spec §11: the spec anticipated the `pmtiles://` protocol
// shim reading the archive directly. The deployed tile service is `go-pmtiles
// serve`, which already unrolls range requests server-side and exposes plain
// {z}/{x}/{y}.mvt — so a standard vector source is both simpler and one fewer
// browser dependency. The .pmtiles archive is unchanged; only the access path is.
//
// Glyphs come from the Protomaps open basemap asset host — MapLibre needs SDF font
// atlases and we do not run a glyph server. No commercial map API is involved.

const TILE_BASE = `${location.origin}/tiles/indonesia`;

const C = {
  land: '#14110E',
  landAlt: '#191512',
  green: '#1B2119',
  greenHi: '#212A1E',
  water: '#0C1A20',
  waterLine: '#123039',
  roadMinor: '#2B241D',
  roadMain: '#3A2F24',
  roadTrunk: '#4A3826',
  motorway: '#5A4029',
  boundary: '#463A2E',
  label: '#B8AC9A',
  labelHi: '#EDE6DA',
  halo: '#14110E',
  peak: '#C9743E',
};

export const mapStyle = {
  version: 8,
  glyphs: 'https://protomaps.github.io/basemaps-assets/fonts/{fontstack}/{range}.pbf',
  sources: {
    omt: {
      type: 'vector',
      tiles: [`${TILE_BASE}/{z}/{x}/{y}.mvt`],
      minzoom: 0,
      maxzoom: 14,
      attribution: '© OpenStreetMap contributors',
    },
  },
  layers: [
    { id: 'bg', type: 'background', paint: { 'background-color': C.land } },

    {
      id: 'landcover',
      type: 'fill',
      source: 'omt',
      'source-layer': 'landcover',
      paint: {
        'fill-color': [
          'match', ['get', 'class'],
          'wood', C.greenHi,
          'forest', C.greenHi,
          'grass', C.green,
          'farmland', C.landAlt,
          C.landAlt,
        ],
        'fill-opacity': 0.55,
      },
    },
    {
      id: 'park',
      type: 'fill',
      source: 'omt',
      'source-layer': 'park',
      paint: { 'fill-color': C.green, 'fill-opacity': 0.4 },
    },
    {
      id: 'water',
      type: 'fill',
      source: 'omt',
      'source-layer': 'water',
      paint: { 'fill-color': C.water },
    },
    {
      id: 'waterway',
      type: 'line',
      source: 'omt',
      'source-layer': 'waterway',
      paint: {
        'line-color': C.waterLine,
        'line-width': ['interpolate', ['linear'], ['zoom'], 8, 0.4, 14, 1.6],
      },
    },

    // Roads, thinnest class first so majors draw on top.
    {
      id: 'road-minor',
      type: 'line',
      source: 'omt',
      'source-layer': 'transportation',
      filter: ['in', ['get', 'class'], ['literal', ['minor', 'service', 'track']]],
      minzoom: 11,
      paint: {
        'line-color': C.roadMinor,
        'line-width': ['interpolate', ['linear'], ['zoom'], 11, 0.4, 16, 2.5],
      },
    },
    {
      id: 'road-secondary',
      type: 'line',
      source: 'omt',
      'source-layer': 'transportation',
      filter: ['in', ['get', 'class'], ['literal', ['secondary', 'tertiary']]],
      paint: {
        'line-color': C.roadMain,
        'line-width': ['interpolate', ['linear'], ['zoom'], 7, 0.4, 16, 4],
      },
    },
    {
      id: 'road-primary',
      type: 'line',
      source: 'omt',
      'source-layer': 'transportation',
      filter: ['in', ['get', 'class'], ['literal', ['primary', 'trunk']]],
      paint: {
        'line-color': C.roadTrunk,
        'line-width': ['interpolate', ['linear'], ['zoom'], 6, 0.6, 16, 5],
      },
    },
    {
      id: 'road-motorway',
      type: 'line',
      source: 'omt',
      'source-layer': 'transportation',
      filter: ['==', ['get', 'class'], 'motorway'],
      paint: {
        'line-color': C.motorway,
        'line-width': ['interpolate', ['linear'], ['zoom'], 5, 0.8, 16, 6],
      },
    },

    {
      id: 'boundary',
      type: 'line',
      source: 'omt',
      'source-layer': 'boundary',
      filter: ['<=', ['get', 'admin_level'], 4],
      paint: {
        'line-color': C.boundary,
        'line-width': 0.7,
        'line-dasharray': [3, 2],
        'line-opacity': 0.8,
      },
    },

    // ---- Trip layers. Sources are populated by app.js as the agent works. ----
    {
      id: 'route-glow',
      type: 'line',
      source: 'route',
      layout: { 'line-cap': 'round', 'line-join': 'round' },
      paint: {
        'line-color': '#E4622A',
        'line-width': ['interpolate', ['linear'], ['zoom'], 5, 8, 14, 22],
        'line-opacity': 0.16,
        'line-blur': 8,
      },
    },
    {
      id: 'route-line',
      type: 'line',
      source: 'route',
      layout: { 'line-cap': 'round', 'line-join': 'round' },
      paint: {
        'line-color': '#F07A3C',
        'line-width': ['interpolate', ['linear'], ['zoom'], 5, 1.6, 10, 3, 14, 5],
      },
    },
    {
      id: 'route-active',
      type: 'line',
      source: 'route-active',
      layout: { 'line-cap': 'round', 'line-join': 'round' },
      paint: {
        'line-color': '#FFD9A0',
        'line-width': ['interpolate', ['linear'], ['zoom'], 5, 2.6, 14, 7],
      },
    },

    // Place labels sit above terrain but below the trip markers.
    {
      id: 'place-label',
      type: 'symbol',
      source: 'omt',
      'source-layer': 'place',
      filter: ['in', ['get', 'class'], ['literal', ['city', 'town', 'village']]],
      layout: {
        'text-field': ['coalesce', ['get', 'name:latin'], ['get', 'name']],
        'text-font': ['Noto Sans Regular'],
        'text-size': ['interpolate', ['linear'], ['zoom'], 6, 10, 12, 15],
        'text-max-width': 8,
      },
      paint: {
        'text-color': C.label,
        'text-halo-color': C.halo,
        'text-halo-width': 1.4,
      },
    },
    {
      id: 'peak-label',
      type: 'symbol',
      source: 'omt',
      'source-layer': 'mountain_peak',
      minzoom: 8,
      layout: {
        'text-field': ['coalesce', ['get', 'name:latin'], ['get', 'name']],
        'text-font': ['Noto Sans Italic'],
        'text-size': 11,
        'text-offset': [0, 0.7],
        'text-anchor': 'top',
      },
      paint: {
        'text-color': C.peak,
        'text-halo-color': C.halo,
        'text-halo-width': 1.2,
      },
    },

    {
      id: 'poi-halo',
      type: 'circle',
      source: 'pois',
      paint: {
        'circle-radius': ['interpolate', ['linear'], ['zoom'], 6, 5, 14, 12],
        'circle-color': '#4FA3A5',
        'circle-opacity': 0.14,
      },
    },
    {
      id: 'poi-dot',
      type: 'circle',
      source: 'pois',
      paint: {
        'circle-radius': ['interpolate', ['linear'], ['zoom'], 6, 2.4, 14, 4.5],
        'circle-color': '#7FD4D6',
        'circle-stroke-color': C.land,
        'circle-stroke-width': 1,
      },
    },
    {
      id: 'poi-label',
      type: 'symbol',
      source: 'pois',
      minzoom: 9,
      layout: {
        'text-field': ['get', 'name'],
        'text-font': ['Noto Sans Regular'],
        'text-size': 11,
        'text-offset': [0, 1],
        'text-anchor': 'top',
        'text-optional': true,
      },
      paint: {
        'text-color': '#9FD8D9',
        'text-halo-color': C.halo,
        'text-halo-width': 1.2,
      },
    },
  ],
};

export const EMPTY_FC = { type: 'FeatureCollection', features: [] };
