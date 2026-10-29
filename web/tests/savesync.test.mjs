import assert from 'node:assert/strict';
import test from 'node:test';

import { startSaveSync, uuidv4, validateSave } from '../savesync.js';

test('ids are valid v4 uuids even where crypto.randomUUID does not exist (insecure origins)', () => {
  const insecure = { getRandomValues: (a) => globalThis.crypto.getRandomValues(a) };
  const id = uuidv4(insecure);
  assert.match(id, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  assert.notEqual(id, uuidv4(insecure));
  assert.match(uuidv4(), /^[0-9a-f-]{36}$/);
});

const PICK = 'benilla:/benilla-config/survivors-pick.txt';
const CONFIG = 'benilla:/benilla-config/config.toml';
const CACHE = 'benilla:/benilla-config/cache/realm.tsv';

/** A localStorage stand-in with its own class, so each test patches a prototype of its own. */
function makeStorage() {
  class FakeStorage {
    constructor() { this.m = new Map(); }
    get length() { return this.m.size; }
    key(i) { return [...this.m.keys()][i] ?? null; }
    getItem(k) { return this.m.has(k) ? this.m.get(k) : null; }
    setItem(k, v) { this.m.set(String(k), String(v)); }
    removeItem(k) { this.m.delete(k); }
  }
  return { Storage: FakeStorage, storage: new FakeStorage() };
}

/** An in-memory `GET/PUT /saves/{id}`: what survivors-lobby's save.rs does, minus the disk. */
function makeServer({ down = false } = {}) {
  const saves = new Map();
  const log = [];
  const fetch = async (url, init = {}) => {
    const method = init.method ?? 'GET';
    log.push(method);
    if (down) throw new TypeError('network down');
    const id = url.split('/saves/')[1];
    if (method === 'PUT') {
      saves.set(id, init.body);
      return { ok: true, status: 204 };
    }
    if (!saves.has(id)) return { ok: false, status: 404 };
    return { ok: true, status: 200, json: async () => JSON.parse(saves.get(id)) };
  };
  return { saves, log, fetch };
}

const waitFor = async (cond, ms = 1000) => {
  const end = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > end) throw new Error('timed out');
    await new Promise((r) => setTimeout(r, 5));
  }
};

const start = (server, s, extra = {}) =>
  startSaveSync({
    server: 'https://lobby.test',
    fetch: server.fetch,
    storage: s.storage,
    Storage: s.Storage,
    debounceMs: 5,
    window: undefined,
    ...extra,
  });

test('not configured: no request, nothing patched, no id minted', async () => {
  const server = makeServer();
  const s = makeStorage();
  const before = s.Storage.prototype.setItem;
  const sync = await startSaveSync({
    env: {}, search: '', fetch: server.fetch, storage: s.storage, Storage: s.Storage,
  });
  assert.equal(sync, null);
  assert.deepEqual(server.log, []);
  assert.equal(s.Storage.prototype.setItem, before);
  assert.equal(s.storage.length, 0);
});

test('first run with local files: mints an id and uploads only the synced files', async () => {
  const server = makeServer();
  const s = makeStorage();
  s.storage.setItem(PICK, '1 2 0 3');
  s.storage.setItem(CONFIG, 'a = "1"');
  s.storage.setItem(CACHE, 'regenerable');
  s.storage.setItem('someone-elses-key', 'x');
  const sync = await start(server, s);
  await sync.syncNow();
  assert.match(sync.id, /^[0-9a-f-]{36}$/);
  assert.equal(s.storage.getItem('benilla-save-id'), sync.id);
  const blob = JSON.parse(server.saves.get(sync.id));
  assert.equal(validateSave(blob), null);
  assert.deepEqual(blob.files, {
    '/benilla-config/survivors-pick.txt': '1 2 0 3',
    '/benilla-config/config.toml': 'a = "1"',
  });
  assert.equal(sync.status(), 'synced');
});

test('a fresh browser with the same id pulls the save and drops stale local files', async () => {
  const server = makeServer();
  const a = makeStorage();
  a.storage.setItem(PICK, '4 1 1 7');
  const first = await start(server, a);
  await first.syncNow();

  const b = makeStorage();
  b.storage.setItem('benilla-save-id', first.id);
  b.storage.setItem(CONFIG, 'stale local file the server never had');
  b.storage.setItem('someone-elses-key', 'x');
  await start(server, b);
  assert.equal(b.storage.getItem(PICK), '4 1 1 7');
  assert.equal(b.storage.getItem(CONFIG), null, 'in-scope files not in the save are removed');
  assert.equal(b.storage.getItem('someone-elses-key'), 'x', 'foreign keys are untouched');
});

test('the game writing a file is pushed after the debounce; unsynced keys are not', async () => {
  const server = makeServer();
  const s = makeStorage();
  const sync = await start(server, s);
  await sync.syncNow();
  server.log.length = 0;

  s.storage.setItem(CACHE, 'noise');
  s.storage.setItem('someone-elses-key', 'x');
  await new Promise((r) => setTimeout(r, 30));
  assert.deepEqual(server.log, [], 'out-of-scope writes cost no request');

  s.storage.setItem(PICK, '2 0 1 5');
  await waitFor(() => server.log.includes('PUT'));
  await waitFor(() => sync.status() === 'synced');
  assert.equal(JSON.parse(server.saves.get(sync.id)).files['/benilla-config/survivors-pick.txt'], '2 0 1 5');

  s.storage.removeItem(PICK);
  await waitFor(() => !('/benilla-config/survivors-pick.txt' in JSON.parse(server.saves.get(sync.id)).files));
});

test('local newer than the server: local wins and goes up', async () => {
  const server = makeServer();
  const a = makeStorage();
  a.storage.setItem(PICK, 'old');
  const first = await start(server, a, { now: () => 1000 });
  await first.syncNow();

  const b = makeStorage();
  b.storage.setItem('benilla-save-id', first.id);
  b.storage.setItem(PICK, 'newer local');
  b.storage.setItem('benilla-save-at', '5000');
  const second = await start(server, b);
  assert.equal(b.storage.getItem(PICK), 'newer local');
  await second.syncNow();
  assert.equal(JSON.parse(server.saves.get(first.id)).files['/benilla-config/survivors-pick.txt'], 'newer local');
});

test('server unreachable: the game still starts on local state', async () => {
  const server = makeServer({ down: true });
  const s = makeStorage();
  s.storage.setItem(PICK, '1 1 1 1');
  const sync = await start(server, s);
  assert.equal(sync.status(), 'offline');
  assert.equal(s.storage.getItem(PICK), '1 1 1 1');
});

test('import: rejects anything that is not a save, applies a good one and uploads it', async () => {
  const server = makeServer();
  const s = makeStorage();
  const sync = await start(server, s);
  await sync.syncNow();

  assert.equal(sync.importText('nope').ok, false);
  assert.equal(sync.importText(JSON.stringify({ format: 'other', version: 1 })).ok, false);
  const evil = { format: 'aghool-survivors-save', version: 1, savedAt: 1, files: { '/benilla-config/../../etc/x': 'y' } };
  assert.equal(sync.importText(JSON.stringify(evil)).ok, false);
  const outside = { ...evil, files: { '/elsewhere/x': 'y' } };
  assert.equal(sync.importText(JSON.stringify(outside)).ok, false);
  assert.equal(s.storage.getItem('benilla:/elsewhere/x'), null);

  const good = { format: 'aghool-survivors-save', version: 1, savedAt: 1, files: { '/benilla-config/survivors-pick.txt': '5 5 5 5' } };
  assert.deepEqual(sync.importText(JSON.stringify(good)), { ok: true, files: 1 });
  assert.equal(s.storage.getItem(PICK), '5 5 5 5');
  await waitFor(() => JSON.parse(server.saves.get(sync.id)).files['/benilla-config/survivors-pick.txt'] === '5 5 5 5');
});

test('export round-trips through import in another browser', async () => {
  const server = makeServer();
  const a = makeStorage();
  a.storage.setItem(PICK, '9 0 1 2');
  const first = await start(server, a);
  const file = first.exportText();

  const b = makeStorage();
  const second = await start(makeServer(), b);
  assert.deepEqual(second.importText(file), { ok: true, files: 1 });
  assert.equal(b.storage.getItem(PICK), '9 0 1 2');
});
