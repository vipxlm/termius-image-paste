#!/usr/bin/env node
// Collect package license texts; this is not a legal or compatibility review.
import { execFileSync } from 'node:child_process';
import { mkdir, readFile, readdir, realpath, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const noticeName = /^(?:licen[cs]e|copying|copyright|notice)(?:[._-].*)?$/i;
const maxTextBytes = 2 * 1024 * 1024;
const compare = (a, b) => a < b ? -1 : a > b ? 1 : 0;
const inside = (root, target) => {
  const relative = path.relative(root, target);
  return relative !== '..' && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative);
};

export async function createNotices(metadata) {
  if (!Array.isArray(metadata.packages) || !Array.isArray(metadata.workspace_members)) {
    throw new Error('Cargo metadata must contain packages and workspace_members arrays.');
  }
  const ownPackages = new Set(metadata.workspace_members);
  const packages = metadata.packages.filter(pkg => !ownPackages.has(pkg.id)).sort((a, b) =>
    compare(`${a.name}@${a.version}:${a.source ?? ''}:${a.id}`, `${b.name}@${b.version}:${b.source ?? ''}:${b.id}`));
  const sections = [
    'THIRD-PARTY NOTICES',
    'Generated from cargo metadata --locked. Includes resolved Cargo packages across targets,',
    'build dependencies and development dependencies; this can exceed a single binary\'s dependencies.',
    'License expressions are package metadata, not a legal or compatibility determination.',
    'Only declared license_file and top-level LICENSE/LICENCE/COPYING/COPYRIGHT/NOTICE files',
    'inside each package directory are collected. Missing or rejected texts are listed explicitly.',
    'Some crates embed additional notices elsewhere; this collection does not claim completeness.',
    'The project LICENSE must be distributed separately. Workspace packages are excluded here.',
    '',
  ];
  const missing = [];
  for (const pkg of packages) {
    const label = `${pkg.name} ${pkg.version}`;
    const warnings = [];
    const texts = [];
    sections.push('='.repeat(78), label, `License expression: ${pkg.license || 'NOT DECLARED'}`,
      `Source: ${pkg.source || 'local path package (absolute path omitted)'}`,
      `Repository: ${pkg.repository || 'not declared'}`);
    try {
      const root = await realpath(path.dirname(pkg.manifest_path));
      const names = new Set();
      if (pkg.license_file) names.add(pkg.license_file);
      for (const entry of await readdir(root, { withFileTypes: true })) {
        if (noticeName.test(entry.name) && (entry.isFile() || entry.isSymbolicLink())) names.add(entry.name);
      }
      const included = new Set();
      for (const name of [...names].sort(compare)) {
        const candidate = path.resolve(root, name);
        // Check both the lexical path and resolved symlink before reading contents.
        if (!inside(root, candidate)) {
          warnings.push('Declared license_file escapes package directory; not read.');
          continue;
        }
        let resolved;
        try {
          resolved = await realpath(candidate);
          if (!inside(root, resolved)) {
            warnings.push('License/notice symlink escapes package directory; not read.');
            continue;
          }
          if (included.has(resolved)) continue;
          const info = await stat(resolved);
          if (!info.isFile() || info.size > maxTextBytes) {
            warnings.push('License/notice entry is not a regular text file or exceeds 2 MiB; not read.');
            continue;
          }
          const bytes = await readFile(resolved);
          const text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes);
          if (text.includes('\0') || !text.trim()) throw new Error('empty or binary license file');
          included.add(resolved);
          // Keep the original text, including its existing line endings.
          texts.push(`--- ${path.relative(root, candidate).split(path.sep).join('/')} ---\n${text}${text.endsWith('\n') ? '' : '\n'}`);
        } catch {
          warnings.push('Declared or discovered license/notice file is missing, empty, unreadable, or not UTF-8 text.');
        }
      }
    } catch {
      warnings.push('Package manifest directory is unavailable; no files read.');
    }
    if (!texts.length) {
      missing.push(label);
      sections.push('LICENSE TEXT MISSING: no readable in-package license/notice text found.');
    }
    for (const warning of [...new Set(warnings)].sort(compare)) sections.push(`NOTICE: ${warning}`);
    sections.push('', ...texts, '');
  }
  sections.push('='.repeat(78), `Resolved third-party packages: ${packages.length}`,
    `Packages without collected license text: ${missing.length}`,
    ...missing.map(label => `MISSING: ${label}`), '');
  return sections.join('\n');
}

async function main() {
  if (process.argv.length !== 3 || ['--help', '-h'].includes(process.argv[2])) {
    console.log('Usage: node tools/third-party-notices.mjs <output-file>');
    process.exitCode = process.argv.length === 3 ? 0 : 2;
    return;
  }
  const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const result = execFileSync('cargo', ['metadata', '--locked', '--format-version', '1',
    '--manifest-path', path.join(projectRoot, 'src-tauri', 'Cargo.toml')],
  { cwd: projectRoot, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024, stdio: ['ignore', 'pipe', 'inherit'] });
  const contents = await createNotices(JSON.parse(result));
  const output = path.resolve(process.argv[2]);
  await mkdir(path.dirname(output), { recursive: true });
  await writeFile(output, contents, 'utf8');
  console.log(`Third-party notices written: ${output}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { console.error(`Cannot generate third-party notices: ${error.message}`); process.exitCode = 1; });
}
