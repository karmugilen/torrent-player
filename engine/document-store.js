import fs from 'node:fs'

const DEFAULT_PREFETCH_BYTES = 48 * 1024 * 1024

// Android grants access to documents, not filesystem paths. Kotlin opens each
// chosen document; Node duplicates those descriptors in the same process.
// Pieces arriving before attach stay in a bounded memory cache so peer
// connections can prefetch. Missing files are deliberately skipped when a
// torrent piece straddles selected and unselected files.
export default class DocumentStore {
  constructor (chunkLength, { files, length, onStore, maxCacheBytes, onCacheFull }) {
    this.chunkLength = chunkLength
    this.length = length
    this.files = files.map(f => ({ offset: f.offset, length: f.length, fd: null }))
    this.closed = false
    this.attached = false
    this.memoryOnly = false
    this.pending = new Set()
    this.cache = new Map()
    this.cacheBytes = 0
    this.evictionCount = 0
    this.flushing = null
    this.closing = null
    this.maxCacheBytes = Number(maxCacheBytes) > 0 ? Number(maxCacheBytes) : DEFAULT_PREFETCH_BYTES
    this.onCacheFull = typeof onCacheFull === 'function' ? onCacheFull : null
    this.onEvict = null
    if (typeof onStore === 'function') onStore(this)
  }

  // Watch mode deliberately never attaches file descriptors. Verified pieces
  // live in this bounded LRU only; evicted pieces become downloadable again so
  // a player can seek outside the current memory window.
  enableMemory (maxCacheBytes, onEvict) {
    if (this.closed || this.attached || this.memoryOnly) throw new Error('Storage already configured or closed')
    const limit = Number(maxCacheBytes)
    if (!Number.isSafeInteger(limit) || limit < this.chunkLength) throw new Error('Memory limit must hold at least one torrent piece')
    this.maxCacheBytes = limit
    this.memoryOnly = true
    this.onCacheFull = null
    this.onEvict = typeof onEvict === 'function' ? onEvict : null
  }

  attach (descriptors) {
    if (this.closed || this.attached || this.memoryOnly || this.files.some(f => f.fd !== null)) throw new Error('Storage already attached or closed')
    if (!Array.isArray(descriptors) || descriptors.length !== this.files.length) throw new Error('File descriptors do not match torrent')
    const opened = []
    try {
      descriptors.forEach((fd, i) => {
        if (fd === null) return
        if (!Number.isInteger(fd) || fd < 3 || !fs.fstatSync(fd).isFile()) throw new Error('Choose a local, seekable download folder')
        const copy = fs.openSync(`/proc/self/fd/${fd}`, 'r+')
        opened.push(copy)
        this.files[i].fd = copy
      })
    } catch (err) {
      for (const fd of opened) fs.closeSync(fd)
      this.files.forEach(f => { f.fd = null })
      throw err
    }
    this.attached = true
  }

  cachePut (index, buffer) {
    const prev = this.cache.get(index)
    if (prev) {
      this.cache.delete(index)
      this.cacheBytes -= prev.length
    }
    const copy = Buffer.from(buffer)
    this.cache.set(index, copy)
    this.cacheBytes += copy.length
    const evicted = []
    if (this.memoryOnly) {
      while (this.cacheBytes > this.maxCacheBytes && this.cache.size > 1) {
        const oldest = this.cache.keys().next().value
        if (oldest === undefined) break
        this.dropCache(oldest)
        evicted.push(oldest)
        this.evictionCount++
      }
    } else if (this.cacheBytes >= this.maxCacheBytes) {
      this.onCacheFull?.()
    }
    return evicted
  }

  dropCache (index) {
    const prev = this.cache.get(index)
    if (!prev) return
    this.cache.delete(index)
    this.cacheBytes -= prev.length
  }

  async flushCache () {
    if (!this.attached || this.cache.size === 0) return
    if (this.flushing) return this.flushing
    this.flushing = (async () => {
      for (const [index, buf] of this.cache) {
        await this.transfer(buf, index * this.chunkLength, true)
        this.dropCache(index)
      }
    })()
    try { await this.flushing } finally { this.flushing = null }
  }

  flush (cb = () => {}) {
    this.run(() => this.flushCache(), cb)
  }

  run (task, cb) {
    if (this.closed) return queueMicrotask(() => cb(new Error('Storage closed')))
    const work = Promise.resolve().then(task)
    this.pending.add(work)
    work.then(value => cb(null, value), cb).finally(() => this.pending.delete(work))
  }

  async transfer (buffer, start, write) {
    const end = start + buffer.length
    for (const file of this.files) {
      const from = Math.max(start, file.offset)
      const to = Math.min(end, file.offset + file.length)
      if (from >= to) continue
      if (file.fd === null) {
        if (write) continue
        throw new Error('Piece includes an unselected or unavailable file')
      }
      let transferred = 0
      while (transferred < to - from) {
        const count = await new Promise((resolve, reject) => {
          fs[write ? 'write' : 'read'](file.fd, buffer, from - start + transferred,
            to - from - transferred, from - file.offset + transferred,
            (err, bytes) => err ? reject(err) : resolve(bytes))
        })
        if (!count) throw new Error(write ? 'No space left or document is not writable' : 'Piece not downloaded')
        transferred += count
      }
    }
    return buffer
  }

  put (index, buffer, cb = () => {}) {
    let evicted = []
    this.run(async () => {
      const expected = Math.min(this.chunkLength, this.length - index * this.chunkLength)
      if (!Number.isSafeInteger(index) || index < 0 || expected <= 0 || !(buffer instanceof Uint8Array) || buffer.length !== expected) throw new Error('Invalid piece length')
      if (this.attached) {
        await this.flushCache()
        await this.transfer(buffer, index * this.chunkLength, true)
        this.dropCache(index)
        return
      }
      evicted = this.cachePut(index, buffer)
    }, (err, value) => {
      // WebTorrent marks the newly written piece verified inside cb. Notify
      // evictions afterwards so that callback cannot overwrite missing state.
      cb(err, value)
      if (!err) {
        for (const evictedIndex of evicted) {
          try { this.onEvict?.(evictedIndex) } catch {}
        }
      }
    })
  }

  get (index, opts, cb) {
    if (typeof opts === 'function') { cb = opts; opts = {} }
    opts ||= {}
    this.run(async () => {
      const offset = opts.offset ?? 0
      const size = Math.min(this.chunkLength, this.length - index * this.chunkLength)
      const length = opts.length ?? size - offset
      if (!Number.isSafeInteger(index) || index < 0 || size <= 0 ||
          !Number.isSafeInteger(offset) || !Number.isSafeInteger(length) ||
          offset < 0 || length < 0 || offset + length > size) throw new Error('Invalid piece range')
      if (this.attached) await this.flushCache()
      const cached = this.cache.get(index)
      if (cached) {
        if (this.memoryOnly) {
          // Map insertion order doubles as the LRU order.
          this.cache.delete(index)
          this.cache.set(index, cached)
        }
        return cached.subarray(offset, offset + length)
      }
      return this.transfer(Buffer.alloc(length), index * this.chunkLength + offset, false)
    }, cb)
  }

  close (cb = () => {}) {
    this.closed = true
    this.closing ||= (async () => {
      await Promise.allSettled([...this.pending])
      try {
        await this.flushCache()
      } finally {
        this.cache.clear()
        this.cacheBytes = 0
        for (const file of this.files) {
          if (file.fd !== null) { fs.closeSync(file.fd); file.fd = null }
        }
      }
    })()
    this.closing.then(() => cb(null), cb)
  }

  // Android owns document deletion and its explicit user confirmation.
  destroy (cb) { this.close(cb) }
}
