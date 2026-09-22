import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const projectRoot = path.dirname(fileURLToPath(import.meta.url));
const cargoManifest = fs.readFileSync(path.join(projectRoot, "../Cargo.toml"), "utf8");
const version = cargoManifest.match(/^version\s*=\s*"([^"]+)"/m)?.[1];

if (!version) {
  throw new Error("Cargo.toml does not declare a package version");
}

export default defineConfig({
  define: {
    __MARKERUP_VERSION__: JSON.stringify(version),
  },
});
