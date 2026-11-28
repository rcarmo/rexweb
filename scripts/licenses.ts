import { mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

interface CargoPackage {
  id: string;
  name: string;
  version: string;
  manifest_path: string;
  repository: string | null;
  license: string | null;
  license_file: string | null;
}

interface CargoMetadata {
  packages: CargoPackage[];
  resolve: { root: string; nodes: { id: string; dependencies: string[] }[] };
}

const args = Bun.argv.slice(2);
const check = args.length === 1 && args[0] === '--check';
if (args.length && !check) throw new Error('Usage: bun scripts/licenses.ts [--check]');

const root = fileURLToPath(new URL('../', import.meta.url));
const out = join(root, 'licenses', 'rust');
// Dependency metadata reads the registry and lockfile without building into target/.
const cargo = Bun.spawn(['cargo', 'metadata', '--locked', '--format-version', '1'], {
  cwd: root,
  stdout: 'pipe',
  stderr: 'inherit',
});
const raw = await new Response(cargo.stdout).text();
if (await cargo.exited !== 0) throw new Error('cargo metadata failed');
const metadata: CargoMetadata = JSON.parse(raw);
const nodes = new Map(metadata.resolve.nodes.map(node => [node.id, node]));
const reachable = new Set<string>();
function visit(id: string) {
  if (reachable.has(id)) return;
  const node = nodes.get(id);
  if (!node) throw new Error(`Missing Cargo dependency node for ${id}`);
  reachable.add(id);
  node.dependencies.forEach(visit);
}
visit(metadata.resolve.root);
reachable.delete(metadata.resolve.root);

const packages = metadata.packages.filter(pkg => reachable.has(pkg.id))
  .sort((a, b) => `${a.name}${a.version}`.localeCompare(`${b.name}${b.version}`, 'en'));
const files = new Map<string, Buffer | string>();
const rows = ['# Rust dependency notices', '', 'Generated from the locked dependency graph in `Cargo.lock`.', '', '| Crate | Version | Licence | Source | Retained notices |', '| --- | --- | --- | --- | --- |'];
for (const pkg of packages) {
  const safe = `${pkg.name}-${pkg.version}`.replace(/[^A-Za-z0-9_.+-]/g, '_');
  const dir = dirname(pkg.manifest_path);
  const notices = (await readdir(dir, { withFileTypes: true }))
    .filter(entry => entry.isFile() && /^(license|licence|copying|notice)(-|\.|_|$)/i.test(entry.name))
    .map(entry => entry.name).sort();
  const copied: string[] = [];
  for (const name of notices) {
    const target = `${safe}-${name}`;
    files.set(target, await readFile(join(dir, name)));
    copied.push(`[${name}](${target})`);
  }
  const source = pkg.repository ? `[source](${pkg.repository})` : 'crates.io';
  rows.push(`| \`${pkg.name}\` | ${pkg.version} | ${pkg.license ?? pkg.license_file ?? 'see retained notice'} | ${source} | ${copied.join(', ') || 'licence metadata only'} |`);
}
files.set('RUST-DEPENDENCIES.md', rows.join('\n') + '\n');

if (!check) await mkdir(out, { recursive: true });
const stale = (await readdir(out)).filter(name => !files.has(name));
const changed: string[] = [];
for (const [name, expected] of files) {
  const path = join(out, name);
  const current = Bun.file(path);
  const bytes = typeof expected === 'string' ? Buffer.from(expected) : expected;
  if (await current.exists() && Buffer.from(await current.arrayBuffer()).equals(bytes)) continue;
  changed.push(name);
  if (!check) await writeFile(path, bytes);
}
if (check && (stale.length || changed.length)) {
  if (stale.length) console.error(`Stale Rust notices: ${stale.join(', ')}`);
  if (changed.length) console.error(`Missing or changed Rust notices: ${changed.join(', ')}`);
  throw new Error('Regenerate Rust dependency notices with bun scripts/licenses.ts');
}
if (!check) for (const name of stale) await rm(join(out, name));
console.log(`${check ? 'Verified' : 'Updated'} notices for ${packages.length} locked Rust dependencies`);
