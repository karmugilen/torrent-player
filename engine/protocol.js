export const VIDEO_EXT = /\.(mp4|m4v|mkv|webm|mov|avi)$/i

export function pickFile (files, fileIndex) {
  if (!Array.isArray(files) || files.length === 0) return null
  if (Number.isInteger(fileIndex) && fileIndex >= 0 && files[fileIndex]) {
    return files[fileIndex]
  }
  const videos = files.filter(f => VIDEO_EXT.test(f.name || f.path || ''))
  const pool = videos.length ? videos : files
  return pool.reduce((a, b) => (a.length >= b.length ? a : b))
}

export function defaultSelectedIndexes (files) {
  if (!Array.isArray(files) || files.length === 0) return []
  const videos = []
  for (let i = 0; i < files.length; i++) {
    if (VIDEO_EXT.test(files[i].name || files[i].path || '')) videos.push(i)
  }
  return videos.length ? videos : files.map((_, i) => i)
}

export function streamUrl (port, streamPath) {
  if (!streamPath) return null
  const path = streamPath.startsWith('/') ? streamPath : `/${streamPath}`
  return `http://127.0.0.1:${port}${path}`
}

export function fileView (file, index) {
  return {
    index,
    name: file.name,
    path: file.path,
    length: file.length,
    progress: file.done || file.length === 0 ? 1 : Math.min(file.progress ?? 0, 0.999999),
    type: file.type || 'application/octet-stream'
  }
}

export function torrentView (id, torrent) {
  const files = torrent.files
    ? torrent.files.map((f, i) => fileView(f, i))
    : []
  return {
    id,
    infoHash: torrent.infoHash || null,
    name: torrent.name || null,
    magnetURI: torrent.magnetURI || null,
    ready: Boolean(torrent.ready),
    done: files.length > 0 && files.every(f => f.progress === 1),
    paused: Boolean(torrent.paused),
    progress: torrent.progress ?? 0,
    downloadSpeed: torrent.downloadSpeed ?? 0,
    uploadSpeed: torrent.uploadSpeed ?? 0,
    numPeers: torrent.numPeers ?? 0,
    length: torrent.length ?? 0,
    downloaded: torrent.downloaded ?? 0,
    uploaded: torrent.uploaded ?? 0,
    timeRemaining: torrent.timeRemaining ?? null,
    files
  }
}

export async function readJson (req) {
  const chunks = []
  for await (const chunk of req) chunks.push(chunk)
  const raw = Buffer.concat(chunks).toString('utf8').trim()
  if (!raw) return {}
  return JSON.parse(raw)
}

export function sendJson (res, status, body) {
  const payload = JSON.stringify(body)
  res.writeHead(status, {
    'Content-Type': 'application/json; charset=utf-8',
    'Content-Length': Buffer.byteLength(payload)
  })
  res.end(payload)
}

export function parsePath (url) {
  const u = new URL(url, 'http://127.0.0.1')
  const parts = u.pathname.replace(/\/+$/, '').split('/').filter(Boolean)
  return { parts, search: u.searchParams }
}
