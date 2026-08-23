// Bumps the three @dshl/* aggregator packages to VERSION and pins their
// @dshl/* optionalDependencies ranges to ^VERSION.
//
// Same logic as release-plugins.yml's inline node step — extracted so local
// publish scripts (scripts/publish.ps1|publish.sh) run the exact same code.
// 0.x caret semantics: ^0.1.0 does NOT match a 0.2.0 tag, hence pinning.
//
// Usage: node scripts/bump-versions.mjs <version>
import fs from 'node:fs'

const version = process.argv[2]
if (!version || !/^\d+\.\d+\.\d+/.test(version)) {
  console.error('usage: node scripts/bump-versions.mjs <version>')
  process.exit(2)
}

for (const pkg of ['dshl-native', 'dshl-pipe', 'dshl-control']) {
  const p = `plugins/${pkg}/package.json`
  const j = JSON.parse(fs.readFileSync(p))
  j.version = version
  if (j.optionalDependencies) {
    for (const k of Object.keys(j.optionalDependencies)) {
      if (k.startsWith('@dshl/')) j.optionalDependencies[k] = `^${version}`
    }
  }
  fs.writeFileSync(p, JSON.stringify(j, null, 2) + '\n')
  console.log(`bumped ${p} -> ${version}`)
}
