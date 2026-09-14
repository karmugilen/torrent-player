import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import fs from 'node:fs/promises'
import { openSync, ftruncateSync, closeSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import http from 'node:http'

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), '..')
const mainJs = path.join(root, 'main.js')
const fixtureDat = path.join(root, 'fixtures', 'tiny.dat')
const fixtureTorrent = path.join(root, 'fixtures', 'tiny.torrent')

function jsonRequest (port, method, urlPath, body, headers = {}) {
  return new Promise((resolve, reject) => {
    const payload = body === undefined ? '' : JSON.stringify(body)
    const req = http.request({
      host: '127.0.0.1',
      port,
      path: urlPath,
      method,
      headers: {
        'Content-Type': 'application/json',
        'Content-Length': Buffer.byteLength(payload),
        ...headers
      }
    }, res => {
      const chunks = []
      res.on('data', c => chunks.push(c))
      res.on('end', () => {
        const raw = Buffer.concat(chunks).toString('utf8')
        let json = null
        try { json = raw ? JSON.parse(raw) : null } catch { json = { raw } }
        resolve({ status: res.statusCode, json, raw, headers: res.headers })
      })
    })
    req.on('error', reject)
    req.end(payload)
  })
}

function rangeGet (url, range) {
  return new Promise((resolve, reject) => {
    const u = new URL(url)
    const req = http.request({
      host: u.hostname,
      port: u.port,
      path: u.pathname + u.search,
      method: 'GET',
      headers: range ? { Range: range } : {}
    }, res => {
      const chunks = []
      res.on('data', c => chunks.push(c))
      res.on('end', () => resolve({
        status: res.statusCode,
        body: Buffer.concat(chunks),
        headers: res.headers
      }))
    })
    req.on('error', reject)
    req.end()
  })
}

async function startEngine (env, extraFds = []) {
  const child = spawn(process.execPath, [mainJs], {
    cwd: root,
    env: { ...process.env, ...env },
    stdio: ['ignore', 'pipe', 'pipe', ...extraFds]
  })
  let stderr = ''
  child.stderr.on('data', d => { stderr += d.toString() })
  const line = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill('SIGKILL')
      reject(new Error('engine start timeout. stderr=' + stderr))
    }, 20000)
    const onExit = (code) => {
      clearTimeout(timer)
      reject(new Error('engine exited ' + code + ' stderr=' + stderr))
    }
    child.once('exit', onExit)
    child.stdout.on('data', chunk => {
      const text = chunk.toString()
      const match = text.split('\n').find(l => l.includes('"event":"listening"'))
      if (match) {
        clearTimeout(timer)
        child.removeListener('exit', onExit)
        resolve(match)
      }
    })
  })
  const info = JSON.parse(line)
  return { child, info, stderr: () => stderr }
}

function sleep (ms) {
  return new Promise(resolve => setTimeout(resolve, ms))
}

async function pollTorrent (port, id, pred, { tries = 80, ms = 100, label = 'poll' } = {}) {
  let last = null
  for (let i = 0; i < tries; i++) {
    last = await jsonRequest(port, 'GET', `/torrent/${id}`)
    if (last.status === 200 && last.json && pred(last.json)) return last.json
    await sleep(ms)
  }
  throw new Error(`${label} timeout status=${last?.status} body=${JSON.stringify(last?.json)}`)
}

async function listFilesRecursive (dir) {
  const out = []
  async function walk (current) {
    let entries
    try {
      entries = await fs.readdir(current, { withFileTypes: true })
    } catch {
      return
    }
    for (const entry of entries) {
      const full = path.join(current, entry.name)
      if (entry.isDirectory()) await walk(full)
      else out.push(full)
    }
  }
  await walk(dir)
  return out
}

async function startPreparedMagnet (t, seedSource, engineEnv = {}) {
  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'webtor-life-'))
  t.after(async () => { await fs.rm(tmp, { recursive: true, force: true }) })
  const downloadDir = path.join(tmp, 'download')
  await fs.mkdir(downloadDir)
  const seedPath = Buffer.isBuffer(seedSource)
    ? path.join(tmp, 'seed.dat')
    : seedSource
  if (Buffer.isBuffer(seedSource)) await fs.writeFile(seedPath, seedSource)
  const { default: WebTorrent } = await import('webtorrent')
  const seed = new WebTorrent({ dht: false, tracker: false, utp: false, lsd: false, natUpnp: false, natPmp: false })
  t.after(() => new Promise(resolve => seed.destroy(resolve)))
  const seeded = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('seed timeout')), 15000)
    seed.seed(seedPath, { announce: [] }, torrent => {
      clearTimeout(timer)
      resolve(torrent)
    })
  })
  const destPaths = seeded.files.map((_, i) => path.join(tmp, 'dest-' + i + '.dat'))
  const destFds = destPaths.map((dest, i) => {
    const fd = openSync(dest, 'w+')
    ftruncateSync(fd, seeded.files[i].length)
    t.after(() => { try { closeSync(fd) } catch {} })
    return fd
  })
  const [destPath] = destPaths
  const [destFd] = destFds
  const { child, info, stderr } = await startEngine({
    WEBTOR_PATH: downloadDir,
    WEBTOR_SKIP_VERIFY: '0',
    ...engineEnv
  }, destFds)
  t.after(async () => {
    if (!child.killed) {
      try { await jsonRequest(info.ctlPort, 'POST', '/shutdown', {}) } catch {}
      child.kill('SIGKILL')
    }
  })
  const magnet = `magnet:?xt=urn:btih:${seeded.infoHash}&x.pe=127.0.0.1:${seed.torrentPort}`
  const added = await jsonRequest(info.ctlPort, 'POST', '/add', {
    torrentId: magnet,
    prepare: true,
    announce: []
  })
  assert.equal(added.status, 200, JSON.stringify(added.json) + ' stderr=' + stderr())
  const id = added.json.id
  const status = await pollTorrent(info.ctlPort, id, s => s.ready, { label: 'metadata ready' })
  return { tmp, downloadDir, destPath, destFd, destPaths, child, info, stderr, id, status, magnet, seeded }
}

function configureFile (port, id, fileCount, inheritedFd = 3, selected = [0]) {
  const descriptors = Array.from({ length: fileCount }, (_, i) => selected.includes(i) ? inheritedFd : null)
  return jsonRequest(port, 'POST', '/configure', { id, selected, descriptors })
}

async function waitForDestPrefix (destPath, prefix, { tries = 80, ms = 100, label = 'dest prefix' } = {}) {
  let got = Buffer.alloc(0)
  for (let i = 0; i < tries; i++) {
    got = await fs.readFile(destPath)
    if (got.length >= prefix.length && got.subarray(0, prefix.length).equals(prefix)) return got
    await sleep(ms)
  }
  throw new Error(`${label} timeout first bytes=${got.subarray(0, prefix.length).toString('hex')}`)
}

test('stats, 400, 404, add+play range, shutdown', async (t) => {
  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'webtor-'))
  const dl = path.join(tmp, 'dl')
  await fs.mkdir(dl)
  const bytes = await fs.readFile(fixtureDat)
  await fs.copyFile(fixtureDat, path.join(dl, 'tiny.dat'))
  const torrentPath = fixtureTorrent

  const { child, info, stderr } = await startEngine({
    WEBTOR_CTL_PORT: '0',
    WEBTOR_STREAM_PORT: '0',
    WEBTOR_PATH: dl,
    WEBTOR_SKIP_VERIFY: '1'
  })
  t.after(async () => {
    if (!child.killed) {
      try { await jsonRequest(info.ctlPort, 'POST', '/shutdown', {}) } catch {}
      child.kill('SIGKILL')
    }
    await fs.rm(tmp, { recursive: true, force: true })
  })

  const step = async (name, fn) => {
    try {
      return await fn()
    } catch (err) {
      err.message = name + ': ' + err.message + ' stderr=' + stderr()
      throw err
    }
  }

  const stats = await step('stats', () => jsonRequest(info.ctlPort, 'GET', '/stats'))
  assert.equal(stats.status, 200)
  assert.equal(typeof stats.json.ctlPort, 'number')
  assert.equal(typeof stats.json.streamPort, 'number')

  const badJson = await step('add-empty', () => jsonRequest(info.ctlPort, 'POST', '/add', {}))
  assert.equal(badJson.status, 400)

  const missing = await step('missing', () => jsonRequest(info.ctlPort, 'GET', '/torrent/nope'))
  assert.equal(missing.status, 404)

  const added = await step('add', () => jsonRequest(info.ctlPort, 'POST', '/add', { torrentId: torrentPath }))
  assert.equal(added.status, 200, JSON.stringify(added.json))
  assert.ok(added.json.id)

  let ready = false
  for (let i = 0; i < 50; i++) {
    const st = await step('poll-' + i, () => jsonRequest(info.ctlPort, 'GET', `/torrent/${added.json.id}`))
    if (st.json && st.json.ready) {
      ready = true
      assert.ok(st.json.files.length >= 1)
      break
    }
    await new Promise(r => setTimeout(r, 100))
  }
  assert.equal(ready, true)

  const play = await step('play', () => jsonRequest(info.ctlPort, 'POST', '/play', { id: added.json.id }))
  assert.equal(play.status, 200, JSON.stringify(play.json))
  assert.match(play.json.streamUrl, /^http:\/\/127\.0\.0\.1:\d+\//)

  const ranged = await step('range', () => rangeGet(play.json.streamUrl, 'bytes=0-6'))
  assert.ok(ranged.status === 206 || ranged.status === 200, 'range status ' + ranged.status)
  assert.equal(ranged.body.subarray(0, 7).toString(), bytes.subarray(0, 7).toString())

  const halt = await step('shutdown', () => jsonRequest(info.ctlPort, 'POST', '/shutdown', {}))
  assert.equal(halt.status, 200)
  await Promise.race([
    once(child, 'exit'),
    new Promise(r => setTimeout(r, 3000))
  ])
})
test('magnet downloads metadata and bytes from a TCP peer, then streams ranges', { timeout: 20000 }, async t => {
  const { default: WebTorrent } = await import('webtorrent')
  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'webtor-peer-'))
  const seed = new WebTorrent({ dht: false, tracker: false, utp: false, lsd: false, natUpnp: false, natPmp: false })
  t.after(async () => {
    await new Promise(resolve => seed.destroy(resolve))
    await fs.rm(tmp, { recursive: true, force: true })
  })
  const torrent = await new Promise(resolve => seed.seed(fixtureDat, { announce: [] }, resolve))
  const { child, info } = await startEngine({ WEBTOR_PATH: path.join(tmp, 'download'), WEBTOR_SKIP_VERIFY: '0' })
  t.after(() => child.kill('SIGKILL'))
  const magnet = `magnet:?xt=urn:btih:${torrent.infoHash}&x.pe=127.0.0.1:${seed.torrentPort}`
  const added = await jsonRequest(info.ctlPort, 'POST', '/add', { torrentId: magnet, announce: [] })
  assert.equal(added.status, 200)
  const play = await jsonRequest(info.ctlPort, 'POST', '/play', { id: added.json.id })
  assert.equal(play.status, 200, JSON.stringify(play.json))
  const result = await rangeGet(play.json.streamUrl, 'bytes=0-6')
  assert.equal(result.status, 206)
  assert.deepEqual(result.body, (await fs.readFile(fixtureDat)).subarray(0, 7))
  const status = await jsonRequest(info.ctlPort, 'GET', `/torrent/${added.json.id}`)
  assert.ok(status.json.downloaded > 0)
  const duplicate = await jsonRequest(info.ctlPort, 'POST', '/add', { torrentId: magnet })
  assert.equal(duplicate.json.id, added.json.id)
  const invalid = await jsonRequest(info.ctlPort, 'POST', '/add', { torrentId: 'not a torrent' })
  assert.equal(invalid.status, 400)
})

test('prepare remains metadata-only until configure starts the download', { timeout: 20000 }, async t => {
  const bytes = await fs.readFile(fixtureDat)
  const { downloadDir, destPath, info, stderr, id, status } = await startPreparedMagnet(t, fixtureDat)
  const selected = await jsonRequest(info.ctlPort, 'POST', '/select', { id, selected: [0] })
  assert.equal(selected.status, 200, JSON.stringify(selected.json) + ' stderr=' + stderr())
  await sleep(500)
  const pre = await jsonRequest(info.ctlPort, 'GET', `/torrent/${id}`)
  assert.equal(pre.status, 200)
  assert.equal(pre.json.configured, false)
  assert.deepEqual(pre.json.selected, [0])
  assert.equal(pre.json.downloaded, 0)
  assert.ok(pre.json.files.every(file => file.progress === 0))
  const destBefore = await fs.readFile(destPath)
  assert.deepEqual(destBefore.subarray(0, 7), Buffer.alloc(7), 'destination received content before configure')
  const written = []
  for (const file of await listFilesRecursive(downloadDir)) {
    const st = await fs.stat(file)
    if (st.size > 0 || path.basename(file) === 'tiny.dat') written.push(file)
  }
  assert.deepEqual(written, [], 'prepare wrote torrent content under ' + downloadDir)

  const badSelect = await jsonRequest(info.ctlPort, 'POST', '/select', { id, selected: [99] })
  assert.equal(badSelect.status, 400)

  const cfg = await configureFile(info.ctlPort, id, status.files.length)
  assert.equal(cfg.status, 200, JSON.stringify(cfg.json) + ' stderr=' + stderr())
  const got = await waitForDestPrefix(destPath, bytes.subarray(0, 7), { label: 'configure starts download' })
  assert.deepEqual(got.subarray(0, 7), bytes.subarray(0, 7))
  const afterSelect = await jsonRequest(info.ctlPort, 'POST', '/select', { id, selected: [0] })
  assert.equal(afterSelect.status, 409)
})

test('prepare does not write content until configure', { timeout: 20000 }, async t => {
  const bytes = await fs.readFile(fixtureDat)
  const { downloadDir, destPath, info, stderr, id, status } = await startPreparedMagnet(t, fixtureDat)
  const fileCount = status.files.length
  assert.ok(fileCount >= 1)
  assert.equal(status.configured, false)

  const written = []
  for (const file of await listFilesRecursive(downloadDir)) {
    const st = await fs.stat(file)
    if (st.size > 0 || path.basename(file) === 'tiny.dat') written.push(file)
  }
  assert.deepEqual(written, [], 'prepare wrote torrent content under ' + downloadDir)

  const playBefore = await jsonRequest(info.ctlPort, 'POST', '/play', { id })
  assert.equal(playBefore.status, 409, JSON.stringify(playBefore.json) + ' stderr=' + stderr())

  const cfg = await configureFile(info.ctlPort, id, fileCount)
  assert.equal(cfg.status, 200, JSON.stringify(cfg.json) + ' stderr=' + stderr())

  await pollTorrent(info.ctlPort, id, s => s.files.every(f => f.progress === 1), { label: 'download complete' })
  const got = await fs.readFile(destPath)
  assert.ok(got.length >= 7, 'dest file is empty')
  assert.deepEqual(got.subarray(0, 7), bytes.subarray(0, 7))
  if (got.length >= bytes.length) assert.deepEqual(got.subarray(0, bytes.length), bytes)

  const meta = await jsonRequest(info.ctlPort, 'GET', `/metadata/${id}`)
  assert.equal(meta.status, 200, JSON.stringify(meta.json))
  assert.equal(typeof meta.json.torrentData, 'string')
  assert.ok(Buffer.from(meta.json.torrentData, 'base64').length > 0)

  const after = await jsonRequest(info.ctlPort, 'GET', `/torrent/${id}`)
  assert.equal(after.json.configured, true)
  assert.equal(after.json.paused, false)
  const play = await jsonRequest(info.ctlPort, 'POST', '/play', { id })
  assert.equal(play.status, 200, JSON.stringify(play.json) + ' stderr=' + stderr())
  assert.match(play.json.streamUrl, /^http:\/\/127\.0\.0\.1:\d+\//)
})

test('pause stops peer transfers; resume continues', { timeout: 20000 }, async t => {
  const payload = Buffer.alloc(256 * 1024)
  for (let i = 0; i < payload.length; i++) payload[i] = (i + 1) % 256
  const prefix = payload.subarray(0, 7)
  const { destPath, info, stderr, id, status } = await startPreparedMagnet(t, payload)
  const cfg = await configureFile(info.ctlPort, id, status.files.length)
  assert.equal(cfg.status, 200, JSON.stringify(cfg.json) + ' stderr=' + stderr())

  // Pause as soon as the dest file has any content (or immediately if already done).
  await waitForDestPrefix(destPath, prefix, { label: 'download started' })
  const paused = await jsonRequest(info.ctlPort, 'POST', '/pause', { id })
  assert.equal(paused.status, 200, JSON.stringify(paused.json))
  const atPause = await pollTorrent(info.ctlPort, id, s => s.paused === true && s.downloadSpeed === 0, {
    tries: 50,
    ms: 100,
    label: 'paused speed 0'
  })
  const downloaded = atPause.downloaded
  await sleep(500)
  const still = await jsonRequest(info.ctlPort, 'GET', `/torrent/${id}`)
  assert.equal(still.json.paused, true)
  const slack = 32 * 1024
  assert.ok(
    still.json.downloaded <= downloaded + slack,
    `downloaded kept climbing ${downloaded} -> ${still.json.downloaded}`
  )

  const resumed = await jsonRequest(info.ctlPort, 'POST', '/resume', { id })
  assert.equal(resumed.status, 200, JSON.stringify(resumed.json))
  const afterResume = await pollTorrent(info.ctlPort, id, s => s.paused === false, { label: 'resumed' })
  assert.equal(afterResume.paused, false)
  await pollTorrent(
    info.ctlPort,
    id,
    s => s.done || s.downloaded > downloaded || s.progress === 1,
    { label: 'resume progress' }
  )
  const got = await waitForDestPrefix(destPath, prefix, { label: 'resume dest' })
  assert.deepEqual(got.subarray(0, 7), prefix)
})

test('resume on unpaused torrent reactivates swarm transfer', { timeout: 20000 }, async t => {
  const payload = Buffer.alloc(256 * 1024)
  for (let i = 0; i < payload.length; i++) payload[i] = (i + 3) % 256
  const prefix = payload.subarray(0, 7)
  const { destPath, info, stderr, id, status } = await startPreparedMagnet(t, payload)
  const cfg = await configureFile(info.ctlPort, id, status.files.length)
  assert.equal(cfg.status, 200, JSON.stringify(cfg.json) + ' stderr=' + stderr())

  const resumed = await jsonRequest(info.ctlPort, 'POST', '/resume', { id })
  assert.equal(resumed.status, 200, JSON.stringify(resumed.json))
  const afterResume = await pollTorrent(info.ctlPort, id, s => s.paused === false, { label: 'resumed' })
  assert.equal(afterResume.paused, false)
  const got = await waitForDestPrefix(destPath, prefix, { label: 'resume unpaused dest' })
  assert.deepEqual(got.subarray(0, 7), prefix)
})

test('remove destroyStore:false keeps destination file', { timeout: 20000 }, async t => {
  const bytes = await fs.readFile(fixtureDat)
  const { destPath, info, stderr, id, status } = await startPreparedMagnet(t, fixtureDat)
  const cfg = await configureFile(info.ctlPort, id, status.files.length)
  assert.equal(cfg.status, 200, JSON.stringify(cfg.json) + ' stderr=' + stderr())
  await pollTorrent(info.ctlPort, id, s => s.files.every(f => f.progress === 1), { label: 'bytes on disk' })
  const before = await fs.readFile(destPath)
  assert.ok(before.length >= 7, 'dest file is empty')
  assert.deepEqual(before.subarray(0, 7), bytes.subarray(0, 7))

  const removed = await jsonRequest(info.ctlPort, 'POST', '/remove', { id, destroyStore: false })
  assert.equal(removed.status, 200, JSON.stringify(removed.json) + ' stderr=' + stderr())
  const st = await fs.stat(destPath)
  assert.ok(st.size > 0)
  const kept = await fs.readFile(destPath)
  assert.deepEqual(kept.subarray(0, 7), bytes.subarray(0, 7))
  const missing = await jsonRequest(info.ctlPort, 'GET', `/torrent/${id}`)
  assert.equal(missing.status, 404)
})

test('POST /settings clamps maxPeers', async t => {
  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'webtor-set-'))
  const { child, info } = await startEngine({ WEBTOR_PATH: tmp, WEBTOR_MAX_PEERS: '12' })
  t.after(async () => {
    try { await jsonRequest(info.ctlPort, 'POST', '/shutdown', {}) } catch {}
    child.kill('SIGKILL')
    await fs.rm(tmp, { recursive: true, force: true })
  })
  const got = await jsonRequest(info.ctlPort, 'GET', '/settings')
  assert.equal(got.status, 200)
  assert.equal(got.json.maxPeers, 12)
  const set = await jsonRequest(info.ctlPort, 'POST', '/settings', { maxPeers: 200 })
  assert.equal(set.status, 200)
  assert.equal(set.json.maxPeers, 80)
})

test('all selected videos finish and restore from disk', { timeout: 60000 }, async t => {
  const source = await fs.mkdtemp(path.join(os.tmpdir(), 'webtor-videos-'))
  t.after(() => fs.rm(source, { recursive: true, force: true }))
  // Include aligned and shared file boundaries across multiple large files.
  const contents = [
    ['01.mp4', Buffer.alloc(20 * 1024 * 1024, 11)],
    ['02.mkv', Buffer.alloc(20 * 1024 * 1024, 29)],
    ['03.webm', Buffer.alloc(20 * 1024 * 1024 + 137, 47)],
    ['readme.txt', Buffer.from('fixture notes')]
  ]
  for (const [name, bytes] of contents) await fs.writeFile(path.join(source, name), bytes)
  const { info, id, status, destPaths, seeded, magnet } = await startPreparedMagnet(t, source)
  const selected = status.files.filter(f => /\.(mp4|mkv|webm)$/.test(f.name)).map(f => f.index)
  assert.equal(selected.length, 3)
  const descriptors = status.files.map((f, i) => selected.includes(i) ? i + 3 : null)
  const configure = await jsonRequest(info.ctlPort, 'POST', '/configure', { id, selected, descriptors })
  assert.equal(configure.status, 200, JSON.stringify(configure.json))
  const done = await pollTorrent(info.ctlPort, id,
    s => selected.every(i => s.files[i].progress === 1), { tries: 300, label: 'all videos complete' })
  assert.deepEqual(done.selected, selected)
  for (const i of selected) {
    const expected = contents.find(([name]) => name === status.files[i].name)[1]
    assert.deepEqual(await fs.readFile(destPaths[i]), expected)
  }
  const metadata = await jsonRequest(info.ctlPort, 'GET', '/metadata/' + id)
  await jsonRequest(info.ctlPort, 'POST', '/remove', { id, destroyStore: false })
  seeded.pause()
  for (const wire of [...seeded.wires]) wire.destroy()
  const restored = await jsonRequest(info.ctlPort, 'POST', '/add', {
    torrentId: magnet, torrentData: metadata.json.torrentData, prepare: true, announce: []
  })
  const restoredId = restored.json.id
  await pollTorrent(info.ctlPort, restoredId, s => s.ready)
  const configured = await jsonRequest(info.ctlPort, 'POST', '/configure', { id: restoredId, selected, descriptors })
  assert.equal(configured.status, 200, JSON.stringify(configured.json))
  // The final piece crosses into the unselected readme. Resume can fetch that
  // boundary piece; fully stored, piece-aligned videos must verify offline.
  const restoredStatus = await jsonRequest(info.ctlPort, 'GET', '/torrent/' + restoredId)
  assert.equal(restoredStatus.json.files[selected[0]].progress, 1)
  assert.equal(restoredStatus.json.files[selected[1]].progress, 1)
})

test('GET /pieces/:id returns telemetry and handles not found', async t => {
  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'webtor-pieces-'))
  const { child, info } = await startEngine({ WEBTOR_PATH: tmp })
  t.after(async () => {
    try { await jsonRequest(info.ctlPort, 'POST', '/shutdown', {}) } catch {}
    child.kill('SIGKILL')
    await fs.rm(tmp, { recursive: true, force: true })
  })

  // 404 for missing torrent
  const missing = await jsonRequest(info.ctlPort, 'GET', '/pieces/non-existent')
  assert.equal(missing.status, 404)

  // Add fixture torrent
  const added = await jsonRequest(info.ctlPort, 'POST', '/add', { torrentId: fixtureTorrent })
  assert.equal(added.status, 200)
  const id = added.json.id
  await pollTorrent(info.ctlPort, id, s => s.ready)

  // Request telemetry
  const piecesResp = await jsonRequest(info.ctlPort, 'GET', `/pieces/${id}?maxBuckets=10`)
  assert.equal(piecesResp.status, 200)
  assert.equal(piecesResp.json.id, id)
  assert.ok(piecesResp.json.totalPieces > 0)
  assert.ok(piecesResp.json.buckets.length > 0 && piecesResp.json.buckets.length <= 10)
  const b0 = piecesResp.json.buckets[0]
  assert.equal(typeof b0.start, 'number')
  assert.equal(typeof b0.end, 'number')
  assert.equal(typeof b0.selected, 'number')
  assert.equal(typeof b0.verified, 'number')
  assert.equal(typeof b0.receiving, 'number')
})

test('control API rejects browser access and malformed request objects', async t => {
  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'webtor-api-'))
  const { child, info } = await startEngine({ WEBTOR_PATH: tmp })
  t.after(async () => {
    try { await jsonRequest(info.ctlPort, 'POST', '/shutdown', {}) } catch {}
    child.kill('SIGKILL')
    await fs.rm(tmp, { recursive: true, force: true })
  })
  for (const headers of [{ Origin: 'https://example.com' }, { Host: 'example.com' }, { 'Sec-Fetch-Site': 'cross-site' }]) {
    const response = await jsonRequest(info.ctlPort, 'POST', '/shutdown', {}, headers)
    assert.equal(response.status, 403)
  }
  for (const endpoint of ['add', 'configure', 'select', 'pause', 'resume', 'play', 'remove', 'settings']) {
    for (const body of [null, [], 'invalid']) {
      const response = await jsonRequest(info.ctlPort, 'POST', '/' + endpoint, body)
      assert.equal(response.status, 400, endpoint + ': ' + JSON.stringify(response.json))
    }
  }
  assert.equal((await jsonRequest(info.ctlPort, 'GET', '/metadata/missing')).status, 404)
  assert.equal((await jsonRequest(info.ctlPort, 'GET', '/stats')).status, 200)
})

test('failed configuration preserves selection and a corrected retry succeeds', { timeout: 20000 }, async t => {
  const { info, id, status } = await startPreparedMagnet(t, fixtureDat)
  const before = await jsonRequest(info.ctlPort, 'GET', '/torrent/' + id)
  for (const descriptors of [[], [1], [999999]]) {
    const invalid = await jsonRequest(info.ctlPort, 'POST', '/configure', { id, selected: [0], descriptors })
    assert.equal(invalid.status, 400, JSON.stringify(invalid.json))
    const after = await jsonRequest(info.ctlPort, 'GET', '/torrent/' + id)
    assert.deepEqual(after.json.selected, before.json.selected)
    assert.equal(after.json.configured, false)
  }
  const responses = await Promise.all([
    configureFile(info.ctlPort, id, status.files.length),
    configureFile(info.ctlPort, id, status.files.length)
  ])
  assert.deepEqual(responses.map(r => r.status).sort(), [200, 409])
  for (const fileIndex of [-1, 999, '0']) {
    const play = await jsonRequest(info.ctlPort, 'POST', '/play', { id, fileIndex })
    assert.equal(play.status, 404)
  }
  await pollTorrent(info.ctlPort, id, s => s.done, { label: 'retry download complete' })
})

test('default playback chooses a selected file', { timeout: 20000 }, async t => {
  const source = await fs.mkdtemp(path.join(os.tmpdir(), 'webtor-play-selection-'))
  t.after(() => fs.rm(source, { recursive: true, force: true }))
  await fs.writeFile(path.join(source, 'large.mp4'), Buffer.alloc(32 * 1024, 23))
  await fs.writeFile(path.join(source, 'small.mp4'), Buffer.alloc(16 * 1024, 61))
  const { info, id, status } = await startPreparedMagnet(t, source)
  const selected = status.files.find(f => f.name === 'small.mp4').index
  const descriptors = status.files.map((_, i) => i === selected ? i + 3 : null)
  const configured = await jsonRequest(info.ctlPort, 'POST', '/configure', { id, selected: [selected], descriptors })
  assert.equal(configured.status, 200, JSON.stringify(configured.json))
  const play = await jsonRequest(info.ctlPort, 'POST', '/play', { id })
  assert.equal(play.status, 200, JSON.stringify(play.json))
  assert.equal(play.json.name, 'small.mp4')
  assert.equal(play.json.fileIndex, selected)
})
