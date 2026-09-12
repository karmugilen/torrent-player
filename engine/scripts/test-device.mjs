// Run with a connected debug install: node scripts/test-device.mjs
import WebTorrent from 'webtorrent'
import { execFileSync } from 'node:child_process'
import http from 'node:http'
import fs from 'node:fs/promises'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'
const fixture = fileURLToPath(new URL('../fixtures/tiny.dat', import.meta.url))
const adb = args => execFileSync('adb', args, { encoding: 'utf8' }).trim()
const seed = new WebTorrent({ dht: false, tracker: false, utp: false, lsd: false, natUpnp: false, natPmp: false })
let id, streamForward, reverse, control
const request = async (route, body) => {
  const r = await fetch(`http://127.0.0.1:${control}${route}`, {
    method: body ? 'POST' : 'GET', body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(40000)
  })
  const json = await r.json()
  if (!r.ok) throw Error(JSON.stringify(json))
  return json
}
try {
  control = adb(['forward', 'tcp:0', 'tcp:18080'])
  const torrent = await new Promise(resolve => seed.seed(fixture, { announce: [] }, resolve))
  reverse = `tcp:${seed.torrentPort}`
  adb(['reverse', reverse, reverse])
  const result = await request('/add', {
    torrentId: `magnet:?xt=urn:btih:${torrent.infoHash}&x.pe=127.0.0.1:${seed.torrentPort}`, announce: []
  })
  id = result.id
  const play = await request('/play', { id })
  const url = new URL(play.streamUrl)
  streamForward = adb(['forward', 'tcp:0', `tcp:${url.port}`])
  const response = await new Promise((resolve, reject) => {
    const req = http.get({ hostname: '127.0.0.1', port: streamForward, path: url.pathname,
      headers: { Host: url.host, Range: 'bytes=0-6' }
    }, res => {
      const chunks = []
      res.on('data', chunk => chunks.push(chunk))
      res.on('error', reject)
      res.on('end', () => resolve({ status: res.statusCode, body: Buffer.concat(chunks) }))
    })
    req.setTimeout(20000, () => req.destroy(new Error('stream timeout')))
    req.on('error', reject)
  })
  assert.equal(response.status, 206)
  assert.deepEqual(response.body, (await fs.readFile(fixture)).subarray(0, 7))
  const state = await request(`/torrent/${id}`)
  assert.ok(state.downloaded > 0)
  console.log(JSON.stringify({ deviceDownload: 'PASS', playbackRange: 'PASS', downloaded: state.downloaded, length: state.length }))
} finally {
  if (id) await request('/remove', { id }).catch(() => {})
  if (streamForward) adb(['forward', '--remove', `tcp:${streamForward}`])
  if (control) adb(['forward', '--remove', `tcp:${control}`])
  if (reverse) adb(['reverse', '--remove', reverse])
  await new Promise(resolve => seed.destroy(resolve))
}
