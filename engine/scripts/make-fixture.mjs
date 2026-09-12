import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import createTorrent from 'create-torrent'

const dir = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'fixtures')
fs.mkdirSync(dir, { recursive: true })
const filePath = path.join(dir, 'tiny.dat')
const bytes = Buffer.from('WEBTOR-TINY-FIXTURE-0123456789\n')
fs.writeFileSync(filePath, bytes)

createTorrent(filePath, { name: 'tiny.dat', announceList: [] }, (err, torrent) => {
  if (err) {
    console.error(err)
    process.exit(1)
  }
  fs.writeFileSync(path.join(dir, 'tiny.torrent'), torrent)
  console.log('wrote', dir, 'torrent', torrent.length)
})
