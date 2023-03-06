import type { UserConfigExport, LibraryOptions } from 'vite';
import { builtinModules } from 'node:module';
import { readFile } from 'node:fs/promises';

export default async function (options: Options): Promise<UserConfigExport> {
  const pkg = await readPkgJson(options.packageName);

  // Rollup will try to bundle dependencies, which we don't want.
  const nodeModules = builtinModules.map((mod: string) => `node:${mod}`);
  const runtimeDeps = Object.keys(pkg.dependencies ?? {});
  const peerDeps = Object.keys(pkg.peerDependencies ?? {});
  const dependencyPatterns = [...runtimeDeps, ...peerDeps].map(
    (depName) => new RegExp(`^${depName}($|/)`)
  );

  return {
    build: {
      lib: {
        entry: options.entry,
        fileName: options.packageName,
        formats: ['es', 'cjs'],
      },
      rollupOptions: {
        external: [...builtinModules, ...nodeModules, ...dependencyPatterns],
      },
    },
  };
}

async function readPkgJson(packageName: string) {
  const path = new URL(
    `./packages/${packageName}/package.json`,
    import.meta.url
  );

  return JSON.parse(await readFile(path, 'utf-8'));
}

interface Options {
  /** Name of the package directory (`packages/<name-goes-here>`). */
  packageName: string;

  /** Path to the entry file(s). */
  entry: LibraryOptions['entry'];
}
