/** Build the existing water-only shell with already installed dependencies.
 * No install, deployment, source mutation, environment-file load or network.
 */
import { createRequire } from 'node:module';
import { pathToFileURL, fileURLToPath } from 'node:url';
import path from 'node:path';
import fs from 'node:fs/promises';
const [consumerArg, outputArg, dependenciesArg = consumerArg] = process.argv.slice(2);
if (!consumerArg || !outputArg) throw new Error('Usage: node build-map.mjs CONSUMER_CHECKOUT NEW_OUTPUT [EXISTING_DEPENDENCY_CHECKOUT]');
const consumer = path.resolve(consumerArg), output = path.resolve(outputArg), dependencies = path.resolve(dependenciesArg);
try { await fs.access(output); throw new Error('Output already exists; preserve prior builds'); } catch (e) { if (e.code !== 'ENOENT') throw e; }
const root = path.join(consumer, 'examples/rainfolk-water');
const require = createRequire(path.join(dependencies, 'package.json'));
const { build } = await import(pathToFileURL(require.resolve('vite')).href);
const plugin = await import(pathToFileURL(require.resolve('vite-plugin-cesium')).href);
const cesium = plugin.default.default ?? plugin.default;
const modules = path.join(dependencies, 'node_modules');
await build({ configFile: false, envFile: false, root, base: './', publicDir: false,
  resolve: { alias: { cesium: path.join(modules, 'cesium/Source/Cesium.js') } },
  plugins: [cesium({cesiumBuildRootPath:path.join(modules,'cesium/Build'),cesiumBuildPath:path.join(modules,'cesium/Build/Cesium/')})],
  build: {target:'es2022',outDir:path.relative(root, output),emptyOutDir:false,sourcemap:false},
});
await fs.access(path.join(output, 'cesium/Cesium.js'));
await fs.mkdir(path.join(output,'notices'),{recursive:true});
const here = path.dirname(fileURLToPath(import.meta.url));
await fs.copyFile(path.join(here,'../../LICENSES/GEV-MIT.txt'),path.join(output,'notices/GEV-MIT.txt'));
await fs.copyFile(path.join(here,'../../LICENSE'),path.join(output,'notices/Rainfolk-MIT.txt'));
for (const name of ['LICENSE.md','ThirdParty.json','ThirdParty.extra.json']) await fs.copyFile(path.join(modules,'cesium',name),path.join(output,'notices','Cesium-'+name));
await fs.writeFile(path.join(output,'notices/ATTRIBUTION.txt'), 'Rainfolk well-source map. Historical USGWD data: CC BY 4.0, original creators and citation at https://www.hydroshare.org/resource/8b02895f02c14dd1a749bcc5584a5c55/.\nGEV layer: original MIT notice retained. Cesium and third-party notices remain in the bundled distribution. Natural Earth imagery is public domain.\nHistorical catalog records are not verified distinct/current wells. Source location accuracy is unknown; approximate datum operations preserve original NAD83 coordinates in provenance.\n');
console.log(JSON.stringify({output,networkRequests:0,dependenciesInstalled:false,sourceMutations:false}));
