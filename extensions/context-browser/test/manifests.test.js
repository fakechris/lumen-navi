import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

for (const name of ["manifest.navi.chromium.json", "manifest.asr.chromium.json", "manifest.safari.json"]) {
  test(`${name} declares bounded optional site access`, async () => {
    const manifest = JSON.parse(await readFile(new URL(`../${name}`, import.meta.url), "utf8"));
    assert.equal(manifest.manifest_version, 3);
    assert.ok(manifest.permissions.includes("nativeMessaging"));
    assert.deepEqual(manifest.optional_host_permissions, ["http://*/*", "https://*/*"]);
    assert.equal(manifest.host_permissions, undefined);
  });
}

test("native host manifest pins one exact extension origin", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "lumen-host-manifest-"));
  const output = path.join(directory, "com.lumen.asr.context_browser.json");
  await execFileAsync(process.execPath, [
    new URL("../scripts/native-host-manifest.mjs", import.meta.url).pathname,
    "com.lumen.asr.context_browser",
    "/Applications/Lumen ASR.app/Contents/MacOS/lumen-asr-context-browser-host",
    "chrome-extension://abcdefghijklmnop/",
    output
  ]);
  const manifest = JSON.parse(await readFile(output, "utf8"));
  assert.deepEqual(manifest.allowed_origins, ["chrome-extension://abcdefghijklmnop/"]);
  assert.equal(manifest.path.startsWith("/"), true);
  assert.equal(manifest.type, "stdio");
});

test("Safari packaging emits a classic single-file service worker", async () => {
  await execFileAsync(process.execPath, [
    new URL("../scripts/package.mjs", import.meta.url).pathname,
    "safari"
  ]);
  const manifest = JSON.parse(
    await readFile(new URL("../dist/safari/manifest.json", import.meta.url), "utf8")
  );
  assert.deepEqual(manifest.background, {
    service_worker: "src/service-worker-safari-bundle.js"
  });
  const bundle = await readFile(
    new URL("../dist/safari/src/service-worker-safari-bundle.js", import.meta.url),
    "utf8"
  );
  assert.equal(/^\s*(?:import|export)\s/m.test(bundle), false);
  assert.match(bundle, /function extractFrame/);
  assert.match(bundle, /function startSafariNativeBridge/);
  assert.match(bundle, /sendNativeMessage\(applicationId, result\)/);
  assert.match(bundle, /startSafariNativeBridge\("ignored-by-safari", "safari"\)/);
});
