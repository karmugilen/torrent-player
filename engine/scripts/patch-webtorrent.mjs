import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const file = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'node_modules', 'webtorrent', 'lib', 'torrent.js')
let src = fs.readFileSync(file, 'utf8')
const from = 'this._debugId = arr2hex(parsedTorrent.infoHash).substring(0, 7)'
const to = 'this._debugId = (typeof parsedTorrent.infoHash === \'string\' ? parsedTorrent.infoHash : arr2hex(parsedTorrent.infoHashBuffer || parsedTorrent.infoHash)).substring(0, 7)'
if (!src.includes(from) && src.includes(to)) {
  console.log('info-hash patch already applied')
}
if (!src.includes(from) && !src.includes(to)) {
  console.error('patch target missing')
  process.exit(1)
}
src = src.split(from).join(to)
fs.writeFileSync(file, src)
console.log('patched webtorrent torrent.js')

// Node 18 and modern Node releases both support createRequire. Import
// assertions were removed from modern Node, while attributes need newer 18.x.
const packageRoot = path.dirname(path.dirname(file))
for (const relative of ['index.js', 'lib/torrent.js', 'lib/webconn.js']) {
  const target = path.join(packageRoot, relative)
  const source = fs.readFileSync(target, 'utf8')
  const patched = source.replace(
    /import info from '(\.\.?\/package\.json)' assert \{ type: 'json' \}/,
    (_, jsonPath) => `import { createRequire } from 'node:module'\nconst info = createRequire(import.meta.url)('${jsonPath}')`
  )
  fs.writeFileSync(target, patched)
}
