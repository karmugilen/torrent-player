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

export function computePieceBuckets (totalPieces, maxBuckets = 256) {
  if (!Number.isInteger(totalPieces) || totalPieces <= 0) return []
  const maxB = Math.max(1, Math.min(Number(maxBuckets) || 256, 1024))
  if (totalPieces <= maxB) {
    const buckets = []
    for (let i = 0; i < totalPieces; i++) {
      buckets.push({ start: i, end: i })
    }
    return buckets
  }
  const bucketSize = Math.ceil(totalPieces / maxB)
  const buckets = []
  for (let start = 0; start < totalPieces; start += bucketSize) {
    const end = Math.min(totalPieces - 1, start + bucketSize - 1)
    buckets.push({ start, end })
  }
  return buckets
}

export function pieceTelemetry (record, { maxBuckets = 256 } = {}) {
  const rec = record?.torrent ? record : { torrent: record }
  const torrent = rec.torrent || record || {}
  const totalPieces = Array.isArray(torrent.pieces) ? torrent.pieces.length : 0
  const maxB = Math.max(1, Math.min(Number(maxBuckets) || 256, 1024))
  const ranges = computePieceBuckets(totalPieces, maxB)

  const pieceLength = Number(torrent.pieceLength) || 0
  const lastPieceLength = Number(torrent.lastPieceLength) || pieceLength

  const files = Array.isArray(torrent.files) ? torrent.files : []
  const prepared = Boolean(rec.prepared)
  const selectedIndices = Array.isArray(rec.selected) ? rec.selected : []

  let allFilesSelected = false
  let selectedFiles = []
  if (prepared) {
    selectedFiles = selectedIndices.map(i => files[i]).filter(Boolean)
  } else if (selectedIndices.length > 0) {
    selectedFiles = selectedIndices.map(i => files[i]).filter(Boolean)
  } else {
    allFilesSelected = true
    selectedFiles = files
  }

  let isPieceSelected
  if (allFilesSelected) {
    isPieceSelected = () => true
  } else if (selectedFiles.length === 0 || pieceLength <= 0) {
    isPieceSelected = () => false
  } else {
    const selectedBitfield = new Uint8Array(totalPieces)
    let runningOffset = 0
    for (let i = 0; i < files.length; i++) {
      const f = files[i]
      const fileOffset = Number.isFinite(f.offset) ? f.offset : runningOffset
      const fileLength = Number(f.length) || 0
      runningOffset = fileOffset + fileLength
      if (!selectedFiles.includes(f) || fileLength <= 0) continue
      const startPiece = Math.max(0, Math.floor(fileOffset / pieceLength))
      const endPiece = Math.min(totalPieces - 1, Math.floor((fileOffset + fileLength - 1) / pieceLength))
      for (let p = startPiece; p <= endPiece; p++) {
        selectedBitfield[p] = 1
      }
    }
    isPieceSelected = p => selectedBitfield[p] === 1
  }

  const receivingSet = new Set()
  const isPaused = Boolean(torrent.paused)
  if (!isPaused) {
    if (Array.isArray(torrent.wires)) {
      for (const wire of torrent.wires) {
        if (Array.isArray(wire.requests)) {
          for (const req of wire.requests) {
            if (Number.isInteger(req?.piece)) {
              receivingSet.add(req.piece)
            }
          }
        }
      }
    }
    if (Array.isArray(torrent._reservations)) {
      for (let p = 0; p < torrent._reservations.length; p++) {
        const res = torrent._reservations[p]
        if (res && res.some(w => w !== null)) {
          receivingSet.add(p)
        }
      }
    }
  }

  const bitfield = torrent.bitfield

  const buckets = ranges.map(({ start, end }) => {
    let verified = 0
    let receiving = 0
    let selected = 0
    for (let p = start; p <= end; p++) {
      const isVer = Boolean(bitfield && bitfield.get(p))
      if (isVer) {
        verified++
      } else if (receivingSet.has(p)) {
        receiving++
      }
      if (isPieceSelected(p)) {
        selected++
      }
    }
    return {
      start,
      end,
      range: [start, end],
      total: end - start + 1,
      selected,
      verified,
      receiving
    }
  })

  return {
    id: rec.id || torrent.id || '',
    infoHash: torrent.infoHash || null,
    generation: rec.generation || 1,
    timestamp: Date.now(),
    totalPieces,
    pieceLength,
    lastPieceLength,
    maxBuckets: maxB,
    buckets
  }
}
