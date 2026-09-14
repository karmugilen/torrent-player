import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { promisify } from 'node:util'
import DocumentStore from '../document-store.js'

function fixture (t, selected = [true, true]) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'webtor-docs-'))
  const paths = ['one', 'two'].map(name => path.join(dir, name))
  const fds = paths.map((file, i) => selected[i] ? fs.openSync(file, 'w+') : null)
  const store = new DocumentStore(4, { length: 7, files: [{ offset: 0, length: 3 }, { offset: 3, length: 4 }], onStore: () => {} })
  t.after(async () => {
    await promisify(store.close.bind(store))()
    for (const fd of fds) if (fd !== null) fs.closeSync(fd)
    fs.rmSync(dir, { recursive: true, force: true })
  })
  return { store, paths, fds, put: promisify(store.put.bind(store)), get: promisify(store.get.bind(store)) }
}

test('document storage writes across files and reads complete and partial pieces', async t => {
  const { store, paths, fds, put, get } = fixture(t)
  store.attach(fds)
  await put(0, Buffer.from('abcd'))
  await put(1, Buffer.from('efg'))
  assert.equal(fs.readFileSync(paths[0], 'utf8'), 'abc')
  assert.equal(fs.readFileSync(paths[1], 'utf8'), 'defg')
  assert.equal((await get(0)).toString(), 'abcd')
  assert.equal((await get(0, { offset: 2, length: 2 })).toString(), 'cd')
  assert.equal((await get(1)).toString(), 'efg')
  await assert.rejects(put(1, Buffer.from('long')), /length/)
  await assert.rejects(get(1, { offset: 2, length: 2 }), /range/)
})

test('unselected documents are skipped on writes but partial selected reads still work', async t => {
  const { store, paths, fds, put, get } = fixture(t, [false, true])
  store.attach(fds)
  await put(0, Buffer.from('abcd'))
  await put(1, Buffer.from('efg'))
  assert.equal(fs.existsSync(paths[0]), false)
  assert.equal(fs.readFileSync(paths[1], 'utf8'), 'defg')
  await assert.rejects(get(0), /unselected/)
  assert.equal((await get(0, { offset: 3, length: 1 })).toString(), 'd')
})

test('invalid descriptor attachment cleans duplicates and permits a corrected retry', async t => {
  const { store, fds, put } = fixture(t)
  assert.throws(() => store.attach([fds[0]]), /match/)
  assert.throws(() => store.attach([fds[0], 1]), /seekable/)
  assert.ok(store.files.every(file => file.fd === null))
  store.attach(fds)
  assert.throws(() => store.attach(fds), /already/)
  await put(0, Buffer.from('abcd'))
})

test('pieces are cached until attach flushes them', async t => {
  const { store, paths, fds, put, get } = fixture(t)
  await put(0, Buffer.from('abcd'))
  await put(1, Buffer.from('efg'))
  assert.equal((await get(0)).toString(), 'abcd')
  assert.equal((await get(1)).toString(), 'efg')
  assert.equal(fs.readFileSync(paths[0], 'utf8'), '')
  assert.equal(fs.readFileSync(paths[1], 'utf8'), '')
  store.attach(fds)
  assert.equal((await get(0)).toString(), 'abcd')
  assert.equal(fs.readFileSync(paths[0], 'utf8'), 'abc')
  assert.equal(fs.readFileSync(paths[1], 'utf8'), 'defg')
})

test('cache-full callback keeps pieces that filled the cache', async t => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'webtor-docs-'))
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }))
  let full = 0
  const store = new DocumentStore(4, {
    length: 7,
    files: [{ offset: 0, length: 3 }, { offset: 3, length: 4 }],
    onStore: () => {},
    maxCacheBytes: 4,
    onCacheFull: () => { full++ }
  })
  t.after(async () => { await promisify(store.close.bind(store))() })
  const put = promisify(store.put.bind(store))
  const get = promisify(store.get.bind(store))
  await put(0, Buffer.from('abcd'))
  assert.equal(full, 1)
  await put(1, Buffer.from('efg'))
  assert.equal((await get(0)).toString(), 'abcd')
  assert.equal((await get(1)).toString(), 'efg')
})

test('close drains outstanding writes, closes duplicates and preserves caller descriptors', async t => {
  const { store, fds, put, get } = fixture(t)
  store.attach(fds)
  const pending = put(0, Buffer.from('abcd'))
  await promisify(store.close.bind(store))()
  await pending
  assert.ok(store.files.every(file => file.fd === null))
  assert.equal(fs.fstatSync(fds[0]).size, 3)
  await assert.rejects(get(0), /closed/)
  await assert.rejects(put(0, Buffer.from('abcd')), /closed/)
})

test('concurrent reads share one cache flush', async t => {
  const { store, fds, put, get } = fixture(t)
  await put(0, Buffer.from('abcd'))
  await put(1, Buffer.from('efg'))
  store.attach(fds)
  let writes = 0
  const transfer = store.transfer.bind(store)
  store.transfer = async (...args) => {
    if (args[2]) writes++
    return transfer(...args)
  }
  const pieces = await Promise.all([get(0), get(1), get(0)])
  assert.deepEqual(pieces.map(b => b.toString()), ['abcd', 'efg', 'abcd'])
  assert.equal(writes, 2, 'Each prefetched piece must be flushed only once')
  assert.equal(store.cacheBytes, 0)
})

test('closing immediately after attach persists cached pieces and pending writes', async t => {
  const { store, fds, paths, put } = fixture(t)
  await put(0, Buffer.from('abcd'))
  store.attach(fds)
  const pending = put(1, Buffer.from('efg'))
  await promisify(store.close.bind(store))()
  await pending
  assert.equal(fs.readFileSync(paths[0], 'utf8'), 'abc')
  assert.equal(fs.readFileSync(paths[1], 'utf8'), 'defg')
  assert.equal(store.cacheBytes, 0)
})

test('invalid piece indexes and fractional ranges cannot read or corrupt storage', async t => {
  const { store, fds, put, get } = fixture(t)
  store.attach(fds)
  for (const index of [-1, 0.5, NaN, Infinity, 2, '0']) {
    await assert.rejects(get(index, { length: 0 }), /range/)
    await assert.rejects(put(index, Buffer.from('abcd')), /length/)
  }
  for (const range of [{ offset: 0.5 }, { offset: NaN }, { length: 1.5 }, { length: NaN }]) {
    await assert.rejects(get(0, range), /range/)
  }
})

function put (store, index, bytes) {
  return new Promise((resolve, reject) => {
    store.put(index, bytes, err => err ? reject(err) : resolve())
  })
}

function get (store, index) {
  return new Promise((resolve, reject) => {
    store.get(index, (err, bytes) => err ? reject(err) : resolve(bytes))
  })
}

test('memory mode is a bounded LRU and reports evictions after writes complete', async () => {
  const evicted = []
  const store = new DocumentStore(4, {
    length: 16,
    files: [{ offset: 0, length: 16 }]
  })
  store.enableMemory(8, index => evicted.push(index))

  await put(store, 0, Buffer.from('aaaa'))
  await put(store, 1, Buffer.from('bbbb'))
  assert.equal((await get(store, 0)).toString(), 'aaaa') // make piece 0 newest
  await put(store, 2, Buffer.from('cccc'))

  assert.deepEqual(evicted, [1])
  assert.equal(store.cacheBytes, 8)
  assert.equal(store.evictionCount, 1)
  assert.equal((await get(store, 0)).toString(), 'aaaa')
  assert.equal((await get(store, 2)).toString(), 'cccc')
  await assert.rejects(get(store, 1), /unselected or unavailable/)
})

test('memory mode cannot later attach download descriptors', () => {
  const store = new DocumentStore(4, {
    length: 4,
    files: [{ offset: 0, length: 4 }]
  })
  store.enableMemory(4)
  assert.throws(() => store.attach([3]), /already attached or closed/)
})
