import http from 'node:http'
import crypto from 'node:crypto'
import fs from 'node:fs'
import process from 'node:process'
import WebTorrent from 'webtorrent'
import parseTorrent from 'parse-torrent'
import DocumentStore from './document-store.js'

process.on('uncaughtException', err => {
  console.error(JSON.stringify({ event: 'uncaught', message: err.message || String(err) }))
})
process.on('unhandledRejection', err => {
  console.error(JSON.stringify({ event: 'unhandledRejection', message: err?.message || String(err) }))
})
import {
  pickFile,
  streamUrl,
  torrentView,
  readJson,
  sendJson,
  parsePath,
  defaultSelectedIndexes
} from './protocol.js'

const CTL_HOST = '127.0.0.1'
const CTL_PORT = Number(process.env.WEBTOR_CTL_PORT || 0)
const STREAM_PORT = Number(process.env.WEBTOR_STREAM_PORT || 0)
const DOWNLOAD_PATH = process.env.WEBTOR_PATH || '/tmp/webtorrent'
const SKIP_VERIFY = process.env.WEBTOR_SKIP_VERIFY === '1'

const records = new Map()
const MAX_PEERS_MIN = 8
const MAX_PEERS_MAX = 80
function clampPeers (n) {
  const v = Number(n)
  if (!Number.isFinite(v)) return 55
  return Math.max(MAX_PEERS_MIN, Math.min(MAX_PEERS_MAX, Math.round(v)))
}
const maxPeers = clampPeers(process.env.WEBTOR_MAX_PEERS || 55)

const client = new WebTorrent({
  utp: false,
  tracker: { wrtc: false },
  maxConns: maxPeers
})

client.on('error', err => {
  console.error(JSON.stringify({ event: 'error', message: err.message || String(err) }))
})

const streamInstance = client.createServer({ hostname: '127.0.0.1' }, 'node')
const streamServer = streamInstance.server

function findRecord (id) {
  if (records.has(id)) return records.get(id)
  for (const rec of records.values()) {
    if (rec.torrent.infoHash === id) return rec
  }
  return null
}

function waitReady (torrent, timeoutMs = 30000) {
  if (torrent.ready) return Promise.resolve()
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => {
      torrent.off('ready', onReady)
      torrent.off('error', onError)
      reject(new Error('torrent metadata timeout'))
    }, timeoutMs)
    function onReady () {
      clearTimeout(t)
      torrent.off('error', onError)
      resolve()
    }
    function onError (err) {
      clearTimeout(t)
      torrent.off('ready', onReady)
      reject(err)
    }
    torrent.once('ready', onReady)
    torrent.once('error', onError)
  })
}

async function handleAdd (req, res) {
  let body
  try {
    body = await readJson(req)
  } catch {
    sendJson(res, 400, { error: 'invalid json' })
    return
  }
  const torrentId = body.torrentId
  if (!torrentId || typeof torrentId !== 'string') {
    sendJson(res, 400, { error: 'torrentId required' })
    return
  }
  let addId = body.torrentData ? Buffer.from(body.torrentData, 'base64') : torrentId
  if (!body.torrentData && !torrentId.startsWith('magnet:') && !/^https?:\/\//.test(torrentId)) {
    try {
      if (fs.existsSync(torrentId) && fs.statSync(torrentId).isFile()) {
        addId = fs.readFileSync(torrentId)
      }
    } catch (err) {
      sendJson(res, 400, { error: err.message || 'cannot read torrent file' })
      return
    }
  }
  let parsed
  if (!/^https?:\/\//.test(torrentId)) {
    try {
      parsed = await parseTorrent(addId)
    } catch (err) {
      sendJson(res, 400, { error: err.message || 'invalid torrent' })
      return
    }
    const existing = findRecord(parsed.infoHash)
    if (existing && !existing.torrent.destroyed) {
      sendJson(res, 200, { id: existing.id, infoHash: parsed.infoHash })
      return
    }
  }
  // Bare magnets otherwise depend entirely on DHT for metadata discovery.
  const announce = Array.isArray(body.announce) ? body.announce
    : torrentId.startsWith('magnet:') && !parsed?.announce?.length
      ? ['udp://tracker.opentrackr.org:1337/announce'] : undefined
  const id = crypto.randomUUID()
  const record = { id, torrent: null, error: null, prepared: body.prepare === true, configured: false, selected: [], documentStore: null, prefetchPaused: false }
  const torrent = client.add(addId, {
    ...(record.prepared
      ? {
          deselect: true,
          store: DocumentStore,
          storeCacheSlots: 0,
          storeOpts: {
            onStore: store => {
              record.documentStore = store
              store.onCacheFull = () => pausePrefetch(record)
            }
          }
        }
      : {}),
    path: DOWNLOAD_PATH,
    skipVerify: record.prepared ? false : SKIP_VERIFY,
    announce
  })
  record.torrent = torrent
  records.set(id, record)
  torrent.once('error', err => {
    record.error = err.message || String(err)
    console.error(JSON.stringify({ event: 'torrent-error', id, message: err.message || String(err) }))
  })
  const startPrefetch = () => {
    if (record.prepared && !record.configured && !record.torrent.destroyed) {
      applySelection(record, defaultSelectedIndexes(record.torrent.files))
    }
  }
  if (torrent.ready) startPrefetch()
  else torrent.once('ready', startPrefetch)
  sendJson(res, 200, { id, infoHash: torrent.infoHash || null })
}

function applySelection (rec, selected) {
  if (!rec?.torrent?.files) return
  const next = [...new Set(selected)].filter(i => Number.isInteger(i) && rec.torrent.files[i])
  rec.torrent.files.forEach((file, i) => {
    const want = next.includes(i)
    const had = rec.prefetchPaused ? false : rec.selected.includes(i)
    if (want && !had) file.select()
    else if (!want && had) file.deselect()
  })
  rec.selected = next
  rec.prefetchPaused = false
}

function pausePrefetch (rec) {
  if (!rec || rec.configured || rec.prefetchPaused || rec.torrent?.destroyed) return
  rec.prefetchPaused = true
  for (const i of rec.selected) rec.torrent.files[i]?.deselect()
}

function flushStore (store) {
  if (!store || typeof store.flush !== 'function') return Promise.resolve()
  return new Promise((resolve, reject) => store.flush(err => err ? reject(err) : resolve()))
}

async function handlePlay (req, res) {
  let body
  try {
    body = await readJson(req)
  } catch {
    sendJson(res, 400, { error: 'invalid json' })
    return
  }
  const rec = findRecord(body.id)
  if (!rec) {
    sendJson(res, 404, { error: 'torrent not found' })
    return
  }
  try {
    if (rec.error || rec.torrent.destroyed) throw new Error(rec.error || 'torrent stopped')
    await waitReady(rec.torrent)
  } catch (err) {
    sendJson(res, 504, { error: err.message || 'not ready' })
    return
  }
  const fileIndex = Number.isInteger(body.fileIndex) ? body.fileIndex : undefined
  const file = pickFile(rec.torrent.files, fileIndex)
  if (!file) {
    sendJson(res, 404, { error: 'no file' })
    return
  }
  if (rec.prepared && (!rec.configured || !rec.selected.includes(rec.torrent.files.indexOf(file)))) {
    sendJson(res, 409, { error: 'Choose a destination and start this file first' })
    return
  }
  if (rec.torrent.paused) { sendJson(res, 409, { error: 'Resume the download before playing' }); return }
  file.select()
  const addr = streamServer.address()
  const port = typeof addr === 'object' && addr ? addr.port : STREAM_PORT
  sendJson(res, 200, {
    id: rec.id,
    fileIndex: rec.torrent.files.indexOf(file),
    name: file.name,
    length: file.length,
    streamUrl: streamUrl(port, file.streamURL)
  })
}

async function handleRemove (req, res) {
  let body
  try {
    body = await readJson(req)
  } catch {
    sendJson(res, 400, { error: 'invalid json' })
    return
  }
  const rec = findRecord(body.id)
  if (!rec) {
    sendJson(res, 404, { error: 'torrent not found' })
    return
  }
  const destroyStore = body.destroyStore !== false
  records.delete(rec.id)
  rec.torrent.destroy({ destroyStore }, err => {
    if (err) {
      sendJson(res, 500, { error: err.message || String(err) })
      return
    }
    sendJson(res, 200, { ok: true, id: rec.id })
  })
}

function handleStats (res) {
  const addr = streamServer.address()
  const ctl = ctlServer.address()
  sendJson(res, 200, {
    downloadSpeed: client.downloadSpeed,
    uploadSpeed: client.uploadSpeed,
    progress: client.progress,
    ratio: client.ratio,
    torrents: client.torrents.length,
    ctlPort: typeof ctl === 'object' && ctl ? ctl.port : CTL_PORT,
    streamPort: typeof addr === 'object' && addr ? addr.port : STREAM_PORT,
    path: DOWNLOAD_PATH
  })
}

function handleTorrent (id, res) {
  const rec = findRecord(id)
  if (!rec) {
    sendJson(res, 404, { error: 'torrent not found' })
    return
  }
  const view = torrentView(rec.id, rec.torrent)
  if (rec.prepared) view.done = rec.configured && rec.selected.length > 0 &&
    rec.selected.every(i => view.files[i]?.progress === 1)
  sendJson(res, 200, { ...view, error: rec.error, configured: rec.configured, selected: rec.selected })
}

async function handleConfigure (req, res) {
  const body = await readJson(req)
  const rec = findRecord(body.id)
  if (!rec || rec.error) return sendJson(res, 404, { error: rec?.error || 'torrent not found' })
  await waitReady(rec.torrent)
  if (!rec.prepared || rec.configured) return sendJson(res, 409, { error: 'Storage already configured' })
  const selected = body.selected
  if (!Array.isArray(selected) || !selected.length || selected.some(i => !Number.isInteger(i) || !rec.torrent.files[i])) {
    return sendJson(res, 400, { error: 'Select at least one file' })
  }
  if (!Array.isArray(body.descriptors) || body.descriptors.some((fd, i) => selected.includes(i) !== (fd !== null))) {
    return sendJson(res, 400, { error: 'Selected files do not match storage' })
  }
  // Selections may have been garbage-collected after RAM prefetch completed.
  // Rebuild them after verification instead of trusting rec.selected.
  for (const i of rec.selected) rec.torrent.files[i]?.deselect()
  rec.selected = []
  rec.prefetchPaused = false
  rec.documentStore.attach(body.descriptors)
  await flushStore(rec.documentStore)
  rec.configured = true
  await new Promise((resolve, reject) => rec.torrent.rescanFiles(err => err ? reject(err) : resolve()))
  applySelection(rec, selected)
  sendJson(res, 200, { ok: true })
}

async function handleSelect (req, res) {
  const body = await readJson(req)
  const rec = findRecord(body.id)
  if (!rec || rec.error) return sendJson(res, 404, { error: rec?.error || 'torrent not found' })
  await waitReady(rec.torrent)
  if (rec.configured) return sendJson(res, 409, { error: 'Storage already configured' })
  const selected = body.selected
  if (!Array.isArray(selected) || selected.some(i => !Number.isInteger(i) || !rec.torrent.files[i])) {
    return sendJson(res, 400, { error: 'Invalid file selection' })
  }
  applySelection(rec, selected)
  sendJson(res, 200, { ok: true, selected: rec.selected })
}

async function handlePause (req, res, paused) {
  const { id } = await readJson(req)
  const rec = findRecord(id)
  if (!rec || rec.torrent.destroyed) return sendJson(res, 404, { error: 'torrent not found' })
  if (paused) {
    rec.peerAddresses = Object.keys(rec.torrent._peers || {})
    rec.torrent.pause()
    // WebTorrent.pause only stops new connections. Disconnect existing wires
    // too, otherwise an active download keeps filling storage after Pause.
    for (const wire of [...rec.torrent.wires]) wire.destroy()
  } else {
    rec.torrent.resume()
    for (const peer of rec.peerAddresses || []) rec.torrent.addPeer(peer)
    rec.torrent.discovery?.tracker?.update()
  }
  sendJson(res, 200, { ok: true })
}

async function handleShutdown (res) {
  sendJson(res, 200, { ok: true })
  setTimeout(() => {
    try {
      if (typeof streamInstance.close === 'function') streamInstance.close()
      else streamServer.close()
    } catch {}
    ctlServer.close()
    client.destroy(() => process.exit(0))
    setTimeout(() => process.exit(0), 1500).unref()
  }, 50)
}

const ctlServer = http.createServer(async (req, res) => {
  const { parts } = parsePath(req.url || '/')
  const method = req.method || 'GET'
  try {
    if (method === 'GET' && parts.length === 1 && parts[0] === 'stats') {
      handleStats(res)
      return
    }
    if (method === 'GET' && parts[0] === 'torrent' && parts[1]) {
      handleTorrent(parts[1], res)
      return
    }
    if (method === 'GET' && parts[0] === 'metadata' && parts[1]) {
      const rec = findRecord(parts[1])
      if (!rec?.torrent.ready) return sendJson(res, 409, { error: 'Metadata not ready' })
      return sendJson(res, 200, { torrentData: Buffer.from(rec.torrent.torrentFile).toString('base64') })
    }
    if (method === 'GET' && parts.length === 1 && parts[0] === 'settings') {
      sendJson(res, 200, { maxPeers: client.maxConns })
      return
    }
    if (method === 'POST' && parts.length === 1 && parts[0] === 'settings') {
      const body = await readJson(req)
      client.maxConns = clampPeers(body.maxPeers)
      sendJson(res, 200, { maxPeers: client.maxConns })
      return
    }
    if (method === 'POST' && parts[0] === 'configure') { await handleConfigure(req, res); return }
    if (method === 'POST' && parts[0] === 'select') { await handleSelect(req, res); return }
    if (method === 'POST' && ['pause', 'resume'].includes(parts[0])) { await handlePause(req, res, parts[0] === 'pause'); return }
    if (method === 'POST' && parts.length === 1 && parts[0] === 'add') {
      await handleAdd(req, res)
      return
    }
    if (method === 'POST' && parts.length === 1 && parts[0] === 'play') {
      await handlePlay(req, res)
      return
    }
    if (method === 'POST' && parts.length === 1 && parts[0] === 'remove') {
      await handleRemove(req, res)
      return
    }
    if (method === 'POST' && parts.length === 1 && parts[0] === 'shutdown') {
      await handleShutdown(res)
      return
    }
    sendJson(res, 404, { error: 'not found' })
  } catch (err) {
    sendJson(res, 500, { error: err.message || String(err) })
  }
})

function listen (server, port) {
  return new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(port, CTL_HOST, () => {
      server.removeListener('error', reject)
      resolve(server.address())
    })
  })
}

const ctlAddr = await listen(ctlServer, CTL_PORT)
const streamAddr = await listen(streamServer, STREAM_PORT)

const ready = {
  event: 'listening',
  ctlPort: ctlAddr.port,
  streamPort: streamAddr.port,
  path: DOWNLOAD_PATH
}
console.log(JSON.stringify(ready))
