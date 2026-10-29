// savesync.js — VPS-backed saves for the browser build.
//
// The desktop build keeps its state in `benilla-config/` beside the exe and never touches this.
// The browser build keeps the same files in `localStorage`, one entry per file, keyed
// `benilla:/benilla-config/<path>` (`local_state.rs`'s `web_store`). Without this module that
// state lives only in one browser; with it, the player's progress also lives on the lobby
// server (`survivors-lobby`, `GET/PUT /saves/{id}`), so clearing site data or switching browser
// doesn't lose it, and the player can download it as a file or import one.
//
// It is page-side on purpose: the client's storage calls are synchronous, an HTTP round trip is
// not, and `localStorage` stays the one place the game reads and writes. This module only
// mirrors it — pull before the game starts, push after it writes.
//
//   - Identity: a random UUID this browser mints once and keeps (`benilla-save-id`). It is the
//     save's only credential, like an unlisted link: whoever holds it can read and replace the
//     save, and the server knows no accounts. The player's downloaded file carries no id.
//   - Conflict: last write wins, by the time of the newest change each side knows of.
//   - Inert unless configured: no server URL, no behaviour change (nothing is patched, no
//     request is made). Set `window.__wenilla_env.survivors_server`, or open the page with
//     `?saves=https://host:8099`.

const PREFIX = 'benilla:';
const ID_KEY = 'benilla-save-id'; // not under PREFIX: the id is not part of the save it names
const AT_KEY = 'benilla-save-at'; // ms epoch of the newest change this browser knows of
const FORMAT = 'aghool-survivors-save';
// Regenerable or machine-local state: never worth a round trip.
const NOT_SYNCED = [
  '/benilla-config/cache/',
  '/benilla-config/Logs/',
  '/benilla-config/Diagnostics/',
  '/benilla-config/Screenshots/',
];
const MAX_BYTES = 2 * 1024 * 1024; // the server's cap too
const HYDRATE_MS = 4000; // the game must not wait longer than this for a slow server
const PUSH_MS = 10000;
const RETRY_MS = [5000, 15000, 60000];
const PATH_RE = /^\/benilla-config\/[\p{L}\p{N}_\-. /]+$/u;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const inScope = (key) =>
  key.startsWith(PREFIX) && !NOT_SYNCED.some((skip) => key.startsWith(PREFIX + skip));

/**
 * A random v4 UUID. `crypto.randomUUID` exists only in secure contexts (HTTPS, localhost);
 * `getRandomValues` exists everywhere, so the id never depends on how the page is served.
 */
export function uuidv4(c = globalThis.crypto) {
  if (typeof c.randomUUID === 'function') return c.randomUUID();
  const b = c.getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, '0')).join('');
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

/** A blob is only ever applied if every file in it is a plain text file under the state folder. */
export function validateSave(blob) {
  if (!blob || typeof blob !== 'object' || blob.format !== FORMAT || blob.version !== 1) {
    return 'not an Azeroth Survivors save file';
  }
  if (!Number.isFinite(blob.savedAt)) return 'save has no timestamp';
  const files = blob.files;
  if (!files || typeof files !== 'object' || Array.isArray(files)) return 'save has no files';
  let bytes = 0;
  for (const [path, text] of Object.entries(files)) {
    if (typeof text !== 'string') return `file ${path} is not text`;
    if (!PATH_RE.test(path) || path.split('/').includes('..')) return `bad path ${path}`;
    if (NOT_SYNCED.some((skip) => path.startsWith(skip))) return `unexpected file ${path}`;
    bytes += path.length + text.length;
  }
  return bytes > MAX_BYTES ? 'save is too large' : null;
}

export async function startSaveSync(opts = {}) {
  const env = opts.env ?? globalThis.window?.__wenilla_env ?? {};
  const search = opts.search ?? globalThis.location?.search ?? '';
  const server = String(
    opts.server ?? new URLSearchParams(search).get('saves') ?? env.survivors_server ?? ''
  ).replace(/\/+$/, '');
  if (!server) return null; // not configured: local-only, exactly as before

  const storage = opts.storage ?? globalThis.localStorage;
  const StorageClass = opts.Storage ?? globalThis.Storage;
  const fetchFn = opts.fetch ?? globalThis.fetch.bind(globalThis);
  const now = opts.now ?? Date.now;
  const debounceMs = opts.debounceMs ?? 3000;
  const mintId = opts.randomUUID ?? (() => uuidv4());
  const win = opts.window ?? globalThis.window;
  const listeners = new Set(opts.onStatus ? [opts.onStatus] : []);

  // The unpatched methods: our own writes (applying a pulled save, stamping the time) must not
  // count as the game changing something.
  const proto = StorageClass.prototype;
  const raw = { set: proto.setItem, remove: proto.removeItem };
  const rawSet = (k, v) => raw.set.call(storage, k, v);
  const rawRemove = (k) => raw.remove.call(storage, k);

  let id = storage.getItem(ID_KEY);
  if (!UUID_RE.test(id ?? '')) {
    id = mintId();
    rawSet(ID_KEY, id);
  }
  const url = `${server}/saves/${id}`;

  let status = 'syncing';
  const setStatus = (s) => {
    status = s;
    listeners.forEach((cb) => cb(s));
  };
  let version = 0; // bumped on every local change, so a push knows if it is still current
  let pushedVersion = 0;
  let timer = null;
  let retries = 0;
  let inflight = null;

  const localAt = () => Number(storage.getItem(AT_KEY)) || 0;
  const stampChange = () => {
    version += 1;
    rawSet(AT_KEY, String(now()));
  };

  function snapshot() {
    const files = {};
    for (let i = 0; i < storage.length; i++) {
      const key = storage.key(i);
      if (key !== null && inScope(key)) files[key.slice(PREFIX.length)] = storage.getItem(key);
    }
    return { format: FORMAT, version: 1, savedAt: localAt() || now(), files };
  }

  /** Make the synced part of localStorage equal `blob`; foreign keys are untouched. */
  function apply(blob) {
    const keep = new Set(Object.keys(blob.files).map((p) => PREFIX + p));
    const stale = [];
    for (let i = 0; i < storage.length; i++) {
      const key = storage.key(i);
      if (key !== null && inScope(key) && !keep.has(key)) stale.push(key);
    }
    stale.forEach(rawRemove);
    for (const [path, text] of Object.entries(blob.files)) rawSet(PREFIX + path, text);
  }

  const timed = async (resource, init, ms) => {
    const ctl = new AbortController();
    const t = setTimeout(() => ctl.abort(), ms);
    try {
      return await fetchFn(resource, { ...init, signal: ctl.signal });
    } finally {
      clearTimeout(t);
    }
  };

  async function push() {
    if (inflight) return inflight;
    inflight = (async () => {
      const sending = version;
      const body = JSON.stringify(snapshot());
      try {
        const res = await timed(
          url,
          {
            method: 'PUT',
            headers: { 'content-type': 'application/json' },
            body,
            keepalive: body.length < 60000,
          },
          PUSH_MS
        );
        if (!res.ok) throw new Error(`server answered ${res.status}`);
        pushedVersion = sending;
        retries = 0;
        // A change made while this was in flight is still owed.
        if (version !== pushedVersion) schedule(debounceMs);
        else setStatus('synced');
      } catch {
        setStatus('offline');
        schedule(RETRY_MS[Math.min(retries++, RETRY_MS.length - 1)]);
      } finally {
        inflight = null;
      }
    })();
    return inflight;
  }

  function schedule(ms) {
    clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      push();
    }, ms);
  }

  const dirty = () => version !== pushedVersion;
  const syncNow = async () => {
    clearTimeout(timer);
    timer = null;
    if (inflight) await inflight;
    if (dirty()) await push();
  };

  // --- pull, before the game reads its state -------------------------------------------------
  async function hydrate() {
    let res;
    try {
      res = await timed(url, {}, HYDRATE_MS);
    } catch {
      setStatus('offline');
      return;
    }
    if (res.status === 404) {
      // The server has nothing for this id yet: whatever this browser has is the first upload.
      if (snapshotHasFiles()) {
        stampIfUnstamped();
        schedule(debounceMs);
        version += 1;
      }
      setStatus(snapshotHasFiles() ? 'syncing' : 'synced');
      return;
    }
    let blob;
    try {
      if (!res.ok) throw new Error(String(res.status));
      blob = await res.json();
    } catch {
      setStatus('offline');
      return;
    }
    if (validateSave(blob)) {
      setStatus('offline'); // a blob this client can't read is left alone, never overwritten
      return;
    }
    if (blob.savedAt >= localAt()) {
      apply(blob);
      rawSet(AT_KEY, String(blob.savedAt));
      setStatus('synced');
    } else {
      // This browser's copy is newer than the server's: it wins and goes up.
      version += 1;
      schedule(debounceMs);
      setStatus('syncing');
    }
  }
  const snapshotHasFiles = () => Object.keys(snapshot().files).length > 0;
  const stampIfUnstamped = () => {
    if (!localAt()) rawSet(AT_KEY, String(now()));
  };

  await hydrate();

  // --- push, as the game writes ----------------------------------------------------------------
  const changed = () => {
    stampChange();
    setStatus('syncing');
    schedule(debounceMs);
  };
  proto.setItem = function (k, v) {
    const r = raw.set.call(this, k, v);
    if (this === storage && inScope(String(k))) changed();
    return r;
  };
  proto.removeItem = function (k) {
    const r = raw.remove.call(this, k);
    if (this === storage && inScope(String(k))) changed();
    return r;
  };
  // A tab being hidden or closed is the last chance: flush rather than wait out the debounce.
  const flush = () => {
    if (dirty() && !inflight) syncNow();
  };
  win?.addEventListener?.('pagehide', flush);
  win?.document?.addEventListener?.('visibilitychange', () => {
    if (win.document.visibilityState === 'hidden') flush();
  });

  // --- the player's own copy: download and import ----------------------------------------------
  const exportText = () => JSON.stringify(snapshot(), null, 2);

  function importText(text) {
    let blob;
    try {
      blob = JSON.parse(text);
    } catch {
      return { ok: false, error: 'not valid JSON' };
    }
    const error = validateSave(blob);
    if (error) return { ok: false, error };
    apply(blob);
    changed();
    syncNow();
    return { ok: true, files: Object.keys(blob.files).length };
  }

  function download() {
    const doc = win.document;
    const a = doc.createElement('a');
    a.href = URL.createObjectURL(new Blob([exportText()], { type: 'application/json' }));
    a.download = 'azeroth-survivors-save.json';
    doc.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(a.href), 10000);
  }

  return {
    id,
    status: () => status,
    onStatus: (cb) => {
      listeners.add(cb);
      cb(status);
      return () => listeners.delete(cb);
    },
    syncNow,
    exportText,
    importText,
    download,
  };
}

/** A small corner control: where the save lives, and download / import for the player's own copy. */
export function mountSaveWidget(api, doc = globalThis.document) {
  const text = {
    syncing: 'Saving…',
    synced: 'Progress saved to the server',
    offline: 'Server unreachable: progress is saved on this device',
  };
  const box = doc.createElement('div');
  box.id = 'wenilla-saves';
  box.style.cssText =
    'position:fixed;right:10px;bottom:10px;z-index:9999;display:flex;gap:8px;align-items:center;' +
    'padding:4px 9px;font:11px/1.4 system-ui,sans-serif;color:#e8d8a8;background:rgba(20,14,10,.72);' +
    'border:1px solid rgba(190,150,70,.45);border-radius:4px;opacity:.55;transition:opacity .15s';
  box.addEventListener('mouseenter', () => (box.style.opacity = '1'));
  box.addEventListener('mouseleave', () => (box.style.opacity = '.55'));

  const label = doc.createElement('span');
  const button = (name, title, onClick) => {
    const b = doc.createElement('button');
    b.type = 'button';
    b.textContent = name;
    b.title = title;
    b.style.cssText =
      'font:inherit;color:inherit;background:none;border:1px solid rgba(190,150,70,.45);' +
      'border-radius:3px;padding:1px 6px;cursor:pointer';
    b.addEventListener('click', onClick);
    return b;
  };

  const input = doc.createElement('input');
  input.type = 'file';
  input.accept = 'application/json,.json';
  input.style.display = 'none';
  input.addEventListener('change', async () => {
    const file = input.files?.[0];
    input.value = '';
    if (!file) return;
    if (!globalThis.confirm('Replace your current progress with this save file?')) return;
    const result = api.importText(await file.text());
    if (!result.ok) return globalThis.alert(`That file can't be imported: ${result.error}`);
    await api.syncNow();
    globalThis.location.reload(); // the game reads its saved state at startup
  });

  box.append(
    label,
    button('Download save', 'Download your progress as a file', () => api.download()),
    button('Import save', 'Replace your progress with a downloaded save file', () => input.click()),
    input
  );
  api.onStatus((s) => (label.textContent = text[s] ?? ''));
  doc.body.appendChild(box);
  return box;
}
