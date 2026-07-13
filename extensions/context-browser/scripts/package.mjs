import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const variants = {
  navi: "manifest.navi.chromium.json",
  asr: "manifest.asr.chromium.json",
  safari: "manifest.safari.json"
};
const variant = process.argv[2];
if (!(variant in variants)) {
  throw new Error(`expected one of: ${Object.keys(variants).join(", ")}`);
}

const destination = path.join(root, "dist", variant);
await rm(destination, { recursive: true, force: true });
await mkdir(destination, { recursive: true });
await cp(path.join(root, "src"), path.join(destination, "src"), { recursive: true });
const manifest = JSON.parse(await readFile(path.join(root, variants[variant]), "utf8"));
if (variant === "safari") {
  const sourceRoot = path.join(root, "src");
  const extractor = (await readFile(path.join(sourceRoot, "extractor.js"), "utf8"))
    .replace(/^export /gm, "");
  const bridge = (await readFile(path.join(sourceRoot, "bridge.js"), "utf8"))
    .replace('import { extractFrame } from "./extractor.js";\n', "")
    .replace(/^export /gm, "");
  const worker = (await readFile(path.join(sourceRoot, "service-worker-safari.js"), "utf8"))
    .replace('import { startSafariNativeBridge } from "./bridge.js";\n', "");
  await writeFile(
    path.join(destination, "src", "service-worker-safari-bundle.js"),
    `${extractor}\n${bridge}\n${worker}`
  );
}
await writeFile(path.join(destination, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
process.stdout.write(`${destination}\n`);
