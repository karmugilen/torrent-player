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
