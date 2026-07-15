#!/usr/bin/env node
/**
 * Set apps/desktop/src-tauri/tauri.conf.json version from a git tag (vMAJOR.MINOR.PATCH).
 */

import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

const [, , tag, configArgument = "apps/desktop/src-tauri/tauri.conf.json"] = process.argv;
const stableTagPattern = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

if (!tag || !stableTagPattern.test(tag)) {
  console.error("Usage: set-tauri-version.mjs vMAJOR.MINOR.PATCH [tauri.conf.json]");
  process.exit(2);
}

const configPath = resolve(configArgument);
const version = tag.slice(1);
const config = JSON.parse(await readFile(configPath, "utf8"));

if (typeof config.version !== "string") {
  throw new Error(`${configPath} does not contain a string version field`);
}

config.version = version;
await writeFile(configPath, `${JSON.stringify(config, null, 2)}\n`);
console.log(`Set Tauri bundle version to ${version}`);
