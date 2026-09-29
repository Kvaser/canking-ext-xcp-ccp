// Vite plugin: records every npm package that ends up in the production bundle
// (JS modules and assets such as fonts) together with its license text, so the
// shipped extension can carry the third-party notices those licenses require.
//
// Writes a JSON list to `outFile` (outside dist/, it is not part of the bundle);
// tools/third-party-notices.mjs renders it into THIRD_PARTY_LICENSES.html.
import fs from "node:fs";
import path from "node:path";
import type { Plugin } from "vite";

export interface JsPackageNotice {
  name: string;
  version: string;
  license: string;
  repository?: string;
  licenseTexts: { file: string; text: string }[];
}

const LICENSE_FILE = /^(licen[cs]e|copying|notice|unlicense)(\.|-|$)/i;

/** `/x/node_modules/@scope/pkg/dist/a.js` -> `/x/node_modules/@scope/pkg` */
function packageRoot(id: string): string | undefined {
  const file = id.split("?")[0].replace(/\\/g, "/");
  const marker = "/node_modules/";
  const at = file.lastIndexOf(marker);
  if (at < 0) return undefined;
  const rest = file.slice(at + marker.length).split("/");
  const nameParts = rest[0].startsWith("@") ? rest.slice(0, 2) : rest.slice(0, 1);
  return file.slice(0, at + marker.length) + nameParts.join("/");
}

function readNotice(root: string): JsPackageNotice | undefined {
  const pkgFile = path.join(root, "package.json");
  if (!fs.existsSync(pkgFile)) return undefined;
  const pkg = JSON.parse(fs.readFileSync(pkgFile, "utf8"));
  const repo = typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url;
  const legacy = Array.isArray(pkg.licenses) ? pkg.licenses.map((l: { type: string }) => l.type).join(" OR ") : undefined;
  const licenseTexts = fs
    .readdirSync(root)
    .filter((f) => LICENSE_FILE.test(f) && fs.statSync(path.join(root, f)).isFile())
    .sort()
    .map((f) => ({ file: f, text: fs.readFileSync(path.join(root, f), "utf8") }));
  return {
    name: pkg.name,
    version: pkg.version,
    license: pkg.license ?? legacy ?? "UNKNOWN",
    repository: repo,
    licenseTexts,
  };
}

export default function thirdPartyLicenses(outFile: string): Plugin {
  let root = process.cwd();
  return {
    name: "third-party-licenses",
    apply: "build",
    configResolved(config) {
      root = config.root;
    },
    generateBundle(_options, bundle) {
      const ids = new Set<string>();
      for (const item of Object.values(bundle)) {
        if (item.type === "chunk") {
          item.moduleIds.forEach((id) => ids.add(id));
        } else {
          // Asset names are relative to the project root (fonts pulled in through CSS url()).
          (item.originalFileNames ?? []).forEach((id) => ids.add(path.resolve(root, id)));
        }
      }

      const packageRoots = new Set<string>();
      for (const id of ids) {
        const pkgRoot = packageRoot(id);
        if (pkgRoot) packageRoots.add(pkgRoot);
      }

      const notices = [...packageRoots]
        .map(readNotice)
        .filter((n): n is JsPackageNotice => n !== undefined)
        .sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));

      fs.writeFileSync(path.resolve(root, outFile), JSON.stringify(notices, null, 2) + "\n");
      this.info(`recorded licenses of ${notices.length} bundled npm package(s) in ${path.basename(outFile)}`);
    },
  };
}
