import { execFile } from "node:child_process";
import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const execFileAsync = promisify(execFile);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const variants = {
  asr: {
    appName: "Lumen ASR Context Bridge",
    bundlePrefix: "org.lumen.asr.context",
    appGroup: "group.org.lumen.asr.context",
    origin: "safari-web-extension://org.lumen.asr.context/"
  },
  navi: {
    appName: "Lumen Navi Context Bridge",
    bundlePrefix: "org.lumen.navi.context",
    appGroup: "group.org.lumen.navi.context",
    origin: "safari-web-extension://org.lumen.navi.context/"
  }
};
const variant = process.argv[2] || "asr";
if (!(variant in variants)) throw new Error("expected Safari consumer: asr or navi");
const { appName, bundlePrefix, appGroup, origin } = variants[variant];
const generated = path.join(root, "dist", `safari-macos-${variant}`);

await run(process.execPath, [path.join(root, "scripts", "package.mjs"), "safari"]);
await rm(generated, { recursive: true, force: true });
await mkdir(generated, { recursive: true });
await run("xcrun", [
  "safari-web-extension-converter",
  path.join(root, "dist", "safari"),
  "--project-location", generated,
  "--app-name", appName,
  "--bundle-identifier", bundlePrefix,
  "--swift",
  "--macos-only",
  "--copy-resources",
  "--no-open",
  "--no-prompt"
]);

const projectRoot = path.join(generated, appName);
const projectFile = path.join(projectRoot, `${appName}.xcodeproj`, "project.pbxproj");
const project = await readFile(projectFile, "utf8");
const extensionIdentifier = project.match(/PRODUCT_BUNDLE_IDENTIFIER = "([^"]+\.Extension)";/)?.[1];
if (!extensionIdentifier) throw new Error("generated Safari extension bundle identifier was not found");
const templates = path.join(root, "safari");
const appDelegate = (await readFile(path.join(templates, "AppDelegate.swift.template"), "utf8"))
  .replaceAll("__APP_GROUP__", appGroup)
  .replaceAll("__EXTENSION_IDENTIFIER__", extensionIdentifier)
  .replaceAll("__ORIGIN__", origin);
const extensionHandler = (await readFile(
  path.join(templates, "SafariWebExtensionHandler.swift.template"),
  "utf8"
)).replaceAll("__APP_GROUP__", appGroup);
await writeFile(path.join(projectRoot, appName, "AppDelegate.swift"), appDelegate);
await writeFile(
  path.join(projectRoot, `${appName} Extension`, "SafariWebExtensionHandler.swift"),
  extensionHandler
);
const applicationEntitlements = path.join(projectRoot, appName, `${appName}.entitlements`);
const extensionEntitlements = path.join(
  projectRoot,
  `${appName} Extension`,
  `${appName.replaceAll(" ", "_")}_Extension.entitlements`
);
for (const entitlements of [applicationEntitlements, extensionEntitlements]) {
  const plist = await readFile(entitlements, "utf8");
  const applicationGroups = `\t<key>com.apple.security.application-groups</key>\n\t<array>\n\t\t<string>${appGroup}</string>\n\t</array>\n`;
  await writeFile(entitlements, plist.replace("</dict>", `${applicationGroups}</dict>`));
}
const derivedData = path.join(generated, "DerivedData");
await run("xcodebuild", [
  "-project", path.join(projectRoot, `${appName}.xcodeproj`),
  "-scheme", appName,
  "-configuration", "Debug",
  "-derivedDataPath", derivedData,
  "CODE_SIGNING_ALLOWED=NO",
  "build"
]);

const application = path.join(derivedData, "Build", "Products", "Debug", `${appName}.app`);
const codesignIdentity = process.env.LUMEN_SAFARI_CODESIGN_IDENTITY;
if (codesignIdentity) {
  const embeddedExtension = path.join(
    application,
    "Contents",
    "PlugIns",
    `${appName} Extension.appex`
  );
  for (const library of await nestedFiles(application, ".dylib")) {
    await codesign(library, codesignIdentity);
  }
  await codesign(embeddedExtension, codesignIdentity, extensionEntitlements);
  await codesign(application, codesignIdentity, applicationEntitlements);
  await run("codesign", ["--verify", "--deep", "--strict", "--verbose=2", application]);
}

process.stdout.write(`${application}\n`);

async function nestedFiles(directory, suffix) {
  const matches = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const candidate = path.join(directory, entry.name);
    if (entry.isDirectory()) matches.push(...await nestedFiles(candidate, suffix));
    else if (entry.isFile() && entry.name.endsWith(suffix)) matches.push(candidate);
  }
  return matches;
}

async function codesign(target, identity, entitlements) {
  const arguments_ = ["--force", "--sign", identity, "--timestamp=none", "--generate-entitlement-der"];
  if (entitlements) arguments_.push("--entitlements", entitlements);
  arguments_.push(target);
  await run("codesign", arguments_);
}

async function run(command, arguments_) {
  try {
    await execFileAsync(command, arguments_, { maxBuffer: 64 * 1024 * 1024 });
  } catch (error) {
    if (error.stdout) process.stderr.write(error.stdout);
    if (error.stderr) process.stderr.write(error.stderr);
    throw error;
  }
}
