// Build the dshl-native napi-rs addon and drop a `.node` beside the plugin.
//
// Produces `native/dshl-native.<platform>-<arch>.node` (gitignored), which the
// plugin's `src/loader.js` resolves before falling back to the per-platform
// `@dshl/native-*` optionalDependency packages.
//
// Usage: node scripts/build-native.mjs [--release]
import { execSync } from 'node:child_process'
import { copyFileSync, mkdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const root = join(here, '..', '..', '..')
const release = process.argv.includes('--release')

const targetDir = join(root, 'target', release ? 'release' : 'debug')
const outDir = join(here, '..', 'native')
mkdirSync(outDir, { recursive: true })

execSync(`cargo build -p dshl-native ${release ? '--release' : ''}`, {
  cwd: root,
  stdio: 'inherit',
})

const artifactName = process.platform === 'win32' ? 'dshl_native.dll'
  : process.platform === 'darwin' ? 'libdshl_native.dylib'
  : 'libdshl_native.so'

const outName = `dshl-native.${process.platform}-${process.arch}.node`
copyFileSync(join(targetDir, artifactName), join(outDir, outName))
console.log(`built ${outName}`)
