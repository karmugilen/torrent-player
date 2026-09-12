import { test } from 'node:test'
import assert from 'node:assert/strict'
import { pickFile, streamUrl, torrentView, fileView, parsePath, defaultSelectedIndexes } from '../protocol.js'

test('pickFile uses explicit index', () => {
  const files = [
    { name: 'a.txt', length: 10 },
    { name: 'b.mp4', length: 5 }
  ]
  assert.equal(pickFile(files, 0).name, 'a.txt')
})

test('pickFile prefers largest video', () => {
  const files = [
    { name: 'readme.txt', length: 9999 },
    { name: 'clip.mp4', length: 100 },
    { name: 'movie.mkv', length: 500 }
  ]
  assert.equal(pickFile(files).name, 'movie.mkv')
})

test('pickFile falls back to largest file', () => {
  const files = [
    { name: 'a.bin', length: 3 },
    { name: 'b.bin', length: 9 }
  ]
  assert.equal(pickFile(files).name, 'b.bin')
})

test('pickFile empty is null', () => {
  assert.equal(pickFile([]), null)
  assert.equal(pickFile(null), null)
})

test('defaultSelectedIndexes prefers videos', () => {
  assert.deepEqual(
    defaultSelectedIndexes([{ name: 'a.txt' }, { name: 'b.mp4' }, { name: 'c.mkv' }]),
    [1, 2]
  )
})

test('defaultSelectedIndexes falls back to every file', () => {
  assert.deepEqual(defaultSelectedIndexes([{ name: 'a.bin' }, { name: 'b.bin' }]), [0, 1])
  assert.deepEqual(defaultSelectedIndexes([]), [])
})

test('streamUrl joins path', () => {
  assert.equal(
    streamUrl(8000, '/webtorrent/abc/video.mp4'),
    'http://127.0.0.1:8000/webtorrent/abc/video.mp4'
  )
})

test('parsePath splits torrent id', () => {
  assert.deepEqual(parsePath('/torrent/deadbeef').parts, ['torrent', 'deadbeef'])
})

test('torrentView maps files', () => {
  const view = torrentView('id-1', {
    infoHash: 'abc',
    name: 't',
    ready: true,
    files: [{ name: 'a.mp4', path: 'a.mp4', length: 12, progress: 1, type: 'video/mp4' }]
  })
  assert.equal(view.id, 'id-1')
  assert.equal(view.files[0].index, 0)
  assert.equal(fileView(view.files[0], 0).name, 'a.mp4')
})

test('completed files report full progress even at an exact piece boundary', () => {
  assert.equal(fileView({ length: 16384, done: true, progress: 0 }, 0).progress, 1)
  assert.equal(fileView({ length: 0, done: true, progress: 0 }, 0).progress, 1)
  assert.ok(fileView({ length: 16384, done: false, progress: 1 }, 0).progress < 1)
})
