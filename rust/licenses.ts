import { mkdir, readdir, copyFile, rm, writeFile } from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';

const root = new URL('.', import.meta.url).pathname;
const backend = join(root, 'backend');
const out = join(root, 'licenses');
const metadata = await new Response(Bun.spawn(['cargo', 'metadata', '--locked', '--format-version', '1'], { cwd: backend, stdout: 'pipe' }).stdout).json();
const reachable = new Set<string>();
function visit(id: string) {
  if (reachable.has(id)) return;
  reachable.add(id);
  const node = metadata.resolve.nodes.find((entry: any) => entry.id === id);
  node?.dependencies.forEach(visit);
}
visit(metadata.resolve.root);
reachable.delete(metadata.resolve.root);
const packages = metadata.packages.filter((pkg: any) => reachable.has(pkg.id)).sort((a: any, b: any) => `${a.name}${a.version}`.localeCompare(`${b.name}${b.version}`));
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });
const rows = ['# Rust dependency notices', '', `Generated from the locked ${metadata.resolve.root} dependency graph.`, '', '| Crate | Version | Licence | Source | Retained notices |', '| --- | --- | --- | --- | --- |'];
for (const pkg of packages) {
  const safe = `${pkg.name}-${pkg.version}`.replace(/[^A-Za-z0-9_.+-]/g, '_');
  const dir = dirname(pkg.manifest_path);
  const files = (await readdir(dir, { withFileTypes: true })).filter(entry => entry.isFile() && /^(license|licence|copying|notice)(-|\.|_|$)/i.test(entry.name)).map(entry => entry.name).sort();
  const copied: string[] = [];
  for (const name of files) {
    const target = `${safe}-${basename(name)}`;
    await copyFile(join(dir, name), join(out, target));
    copied.push(`[${name}](${target})`);
  }
  const source = pkg.repository ? `[source](${pkg.repository})` : 'crates.io';
  rows.push(`| \`${pkg.name}\` | ${pkg.version} | ${pkg.license ?? pkg.license_file ?? 'see retained notice'} | ${source} | ${copied.join(', ') || 'licence metadata only'} |`);
}
await writeFile(join(out, 'RUST-DEPENDENCIES.md'), rows.join('\n') + '\n');
console.log(`Wrote notices for ${packages.length} locked Rust dependencies to ${out}`);
