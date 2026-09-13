import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  pickFile,
  streamUrl,
  torrentView,
  fileView,
  parsePath,
  defaultSelectedIndexes,
  computePieceBuckets,
  pieceTelemetry
} from '../protocol.js'

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

test('computePieceBuckets returns 1-piece buckets when totalPieces <= maxBuckets', () => {
  const buckets = computePieceBuckets(12, 256)
  assert.equal(buckets.length, 12)
  for (let i = 0; i < 12; i++) {
    assert.deepEqual(buckets[i], { start: i, end: i })
  }
})

test('computePieceBuckets partitions into at most maxBuckets nonempty ranges using ceil(totalPieces / maxBuckets)', () => {
  const buckets = computePieceBuckets(12, 5)
  // bucketSize = Math.ceil(12 / 5) = 3
  assert.equal(buckets.length, 4)
  assert.deepEqual(buckets, [
    { start: 0, end: 2 },
    { start: 3, end: 5 },
    { start: 6, end: 8 },
    { start: 9, end: 11 }
  ])
})

test('computePieceBuckets handles 50,000 pieces with maxBuckets 256', () => {
  const buckets = computePieceBuckets(50000, 256)
  // bucketSize = Math.ceil(50000 / 256) = 196
  // total buckets = Math.ceil(50000 / 196) = 256
  assert.ok(buckets.length <= 256)
  assert.equal(buckets[0].start, 0)
  assert.equal(buckets[0].end, 195)
  assert.equal(buckets[buckets.length - 1].end, 49999)
  // contiguous check
  for (let i = 1; i < buckets.length; i++) {
    assert.equal(buckets[i].start, buckets[i - 1].end + 1)
  }
})

test('computePieceBuckets returns empty for zero or negative pieces', () => {
  assert.deepEqual(computePieceBuckets(0), [])
  assert.deepEqual(computePieceBuckets(-1), [])
})

test('pieceTelemetry maps verified pieces from bitfield and excludes verified from receiving', () => {
  const bitfield = {
    get (i) {
      return i === 0 || i === 2 // pieces 0 and 2 are verified
    }
  }
  const wires = [
    {
      requests: [
        { piece: 0 }, // already verified -> receiving must exclude this!
        { piece: 1 }, // receiving
        { piece: 3 }  // receiving
      ]
    }
  ]
  const record = {
    id: 't-1',
    generation: 2,
    prepared: false,
    selected: [],
    torrent: {
      id: 't-1',
      infoHash: 'hash-1',
      pieces: new Array(4),
      pieceLength: 1024,
      lastPieceLength: 512,
      paused: false,
      bitfield,
      wires
    }
  }

  const telemetry = pieceTelemetry(record, { maxBuckets: 4 })
  assert.equal(telemetry.id, 't-1')
  assert.equal(telemetry.infoHash, 'hash-1')
  assert.equal(telemetry.generation, 2)
  assert.equal(telemetry.totalPieces, 4)
  assert.equal(telemetry.buckets.length, 4)

  // Piece 0: verified=1, receiving=0 (excluded because verified), selected=1 (default all)
  assert.deepEqual(telemetry.buckets[0], {
    start: 0,
    end: 0,
    range: [0, 0],
    total: 1,
    selected: 1,
    verified: 1,
    receiving: 0
  })
  // Piece 1: verified=0, receiving=1, selected=1
  assert.deepEqual(telemetry.buckets[1], {
    start: 1,
    end: 1,
    range: [1, 1],
    total: 1,
    selected: 1,
    verified: 0,
    receiving: 1
  })
  // Piece 2: verified=1, receiving=0, selected=1
  assert.deepEqual(telemetry.buckets[2], {
    start: 2,
    end: 2,
    range: [2, 2],
    total: 1,
    selected: 1,
    verified: 1,
    receiving: 0
  })
  // Piece 3: verified=0, receiving=1, selected=1
  assert.deepEqual(telemetry.buckets[3], {
    start: 3,
    end: 3,
    range: [3, 3],
    total: 1,
    selected: 1,
    verified: 0,
    receiving: 1
  })
})

test('pieceTelemetry marks boundary pieces selected when any selected file overlaps them', () => {
  // 2 files of length 1500 each, pieceLength 1000.
  // Total 3000 bytes = 3 pieces (0, 1, 2).
  // File 0: offset 0, length 1500 -> spans piece 0 and 1.
  // File 1: offset 1500, length 1500 -> spans piece 1 and 2.
  const files = [
    { name: 'file0.dat', offset: 0, length: 1500 },
    { name: 'file1.dat', offset: 1500, length: 1500 }
  ]
  const record = {
    id: 't-bound',
    prepared: true,
    selected: [0], // only file 0 selected
    torrent: {
      pieces: new Array(3),
      pieceLength: 1000,
      files,
      paused: false
    }
  }
  const tel = pieceTelemetry(record, { maxBuckets: 3 })
  assert.equal(tel.buckets[0].selected, 1) // piece 0 is in file 0
  assert.equal(tel.buckets[1].selected, 1) // piece 1 is boundary piece overlapped by file 0 -> selected!
  assert.equal(tel.buckets[2].selected, 0) // piece 2 is only in file 1 -> unselected
})

test('pieceTelemetry freezes receiving state when paused', () => {
  const wires = [
    {
      requests: [{ piece: 0 }]
    }
  ]
  const record = {
    id: 't-paused',
    prepared: false,
    selected: [],
    torrent: {
      pieces: new Array(2),
      pieceLength: 1024,
      paused: true,
      wires
    }
  }
  const tel = pieceTelemetry(record, { maxBuckets: 2 })
  assert.equal(tel.buckets[0].receiving, 0)
  assert.equal(tel.buckets[1].receiving, 0)
})
