import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'node_modules')

const utpBinding = path.join(root, 'utp-native', 'lib', 'binding.js')
if (fs.existsSync(utpBinding)) {
  fs.writeFileSync(utpBinding, `const fs = require('fs')
const path = require('path')

function loadNative (name) {
  const dir = process.env.WEBTOR_NATIVE_LIBDIR
  if (!dir) return null
  const file = path.join(dir, name)
  if (!fs.existsSync(file)) return null
  const module = { exports: {}, id: file, filename: file }
  process.dlopen(module, file)
  return module.exports
}

module.exports = loadNative('libutp_native.so') || require('node-gyp-build')(path.join(__dirname, '..'))
`)
  console.log('patched utp-native loader')
}

function writeDatachannelLoader (file, kind) {
  if (!fs.existsSync(file)) return
  if (kind === 'esm') {
    fs.writeFileSync(file, `import cjsPath from 'node:path'
import cjsModule from 'node:module'
import fs from 'node:fs'
const require = cjsModule.createRequire(import.meta.url)

function loadNative () {
  const dir = process.env.WEBTOR_NATIVE_LIBDIR
  if (dir) {
    const file = cjsPath.join(dir, 'libnode_datachannel.so')
    if (fs.existsSync(file)) {
      const module = { exports: {}, id: file, filename: file }
      process.dlopen(module, file)
      return module.exports
    }
  }
  return require('../../../build/Release/node_datachannel.node')
}

const nodeDataChannel = loadNative()
export { nodeDataChannel as default }
`)
  } else {
    fs.writeFileSync(file, `'use strict'
const fs = require('fs')
const path = require('path')

function loadNative () {
  const dir = process.env.WEBTOR_NATIVE_LIBDIR
  if (dir) {
    const file = path.join(dir, 'libnode_datachannel.so')
    if (fs.existsSync(file)) {
      const module = { exports: {}, id: file, filename: file }
      process.dlopen(module, file)
      return module.exports
    }
  }
  return require('../../../build/Release/node_datachannel.node')
}

const nodeDataChannel = loadNative()
exports.default = nodeDataChannel
module.exports = nodeDataChannel
`)
  }
  console.log('patched', path.relative(root, file))
}

writeDatachannelLoader(path.join(root, 'node-datachannel', 'dist', 'esm', 'lib', 'node-datachannel.mjs'), 'esm')
writeDatachannelLoader(path.join(root, 'node-datachannel', 'dist', 'cjs', 'lib', 'node-datachannel.cjs'), 'cjs')
