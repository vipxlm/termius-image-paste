import { readFileSync } from 'node:fs';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const fail = (message) => { throw new Error(message); };
const pkg = JSON.parse(read('package.json'));
const lock = JSON.parse(read('package-lock.json'));
const tauri = JSON.parse(read('src-tauri/tauri.conf.json'));
const cargoPackage = read('src-tauri/Cargo.toml').split(/^\[package\]\s*$/m)[1]?.split(/^\[/m)[0];
const cargoVersion = cargoPackage?.match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1];
const version = pkg.version;
if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version ?? '')) {
  fail('package.json 必须使用稳定版本 X.Y.Z。');
}
for (const [name, actual] of Object.entries({
  'package-lock.json': lock.version,
  'package-lock.json packages[""]': lock.packages?.['']?.version,
  'Cargo.toml': cargoVersion,
  'tauri.conf.json': tauri.version,
})) {
  if (actual !== version) fail(`${name} 版本 ${actual} 与 package.json ${version} 不一致。`);
}
const args = process.argv.slice(2);
if (args.length !== 0) {
  if (args.length !== 2 || args[0] !== '--tag') fail('用法：node tools/check-version.mjs [--tag vX.Y.Z]');
  if (args[1] !== `v${version}`) fail(`发布标签必须严格等于 v${version}，实际为 ${args[1]}。`);
}
console.log(`版本检查通过：${version}${args.length ? `，标签 ${args[1]}` : ''}`);
