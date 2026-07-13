import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const [name, executable, extensionOrigin, output] = process.argv.slice(2);
if (!/^[a-z0-9_]+(?:\.[a-z0-9_]+)+$/.test(name || "")) {
  throw new Error("native host name is invalid");
}
if (!path.isAbsolute(executable || "")) {
  throw new Error("native host executable path must be absolute");
}
if (!/^chrome-extension:\/\/[a-z]+\/$/.test(extensionOrigin || "")) {
  throw new Error("extension origin must be an exact chrome-extension origin ending in /");
}
if (!output) {
  throw new Error("output path is required");
}

const manifest = {
  name,
  description: "Local Lumen browser context bridge",
  path: executable,
  type: "stdio",
  allowed_origins: [extensionOrigin]
};
await mkdir(path.dirname(output), { recursive: true });
await writeFile(output, `${JSON.stringify(manifest, null, 2)}\n`, { mode: 0o600 });
process.stdout.write(`${output}\n`);
