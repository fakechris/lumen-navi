import { createHash, generateKeyPairSync } from "node:crypto";
import { createServer } from "node:http";
import { rmSync } from "node:fs";
import { chmod, copyFile, cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const extensionRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const workspace = path.resolve(extensionRoot, "../..");
const chromium = process.env.LUMEN_CHROMIUM || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const strict = process.env.LUMEN_CHROMIUM_STRICT === "1";
const nativeHostProductDirectory = chromium.includes("Google Chrome") ? "Google/Chrome" : "Chromium";
const nativeHostName = `com.lumen.context_e2e_${process.pid}`;
const temporary = await mkdtemp(path.join(os.tmpdir(), "lumen-browser-e2e-"));
const home = path.join(temporary, "home");
const bridgeRoot = path.join(temporary, "bridge");
const profile = path.join(temporary, "profile");
const unpacked = path.join(temporary, "extension");
await mkdir(home, { recursive: true });
await mkdir(bridgeRoot, { recursive: true });
await cp(path.join(extensionRoot, "src"), path.join(unpacked, "src"), { recursive: true });
const bridgePath = path.join(unpacked, "src/bridge.js");
const workerPath = path.join(unpacked, "src/service-worker-asr.js");

const { publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const publicDer = publicKey.export({ type: "spki", format: "der" });
const alphabet = "abcdefghijklmnop";
const extensionId = [...createHash("sha256").update(publicDer).digest().subarray(0, 16)]
  .flatMap((byte) => [alphabet[byte >> 4], alphabet[byte & 15]])
  .join("");
const extensionOrigin = `chrome-extension://${extensionId}/`;
const manifest = JSON.parse(await readFile(path.join(extensionRoot, "manifest.asr.chromium.json"), "utf8"));
manifest.key = publicDer.toString("base64");
manifest.host_permissions = ["http://127.0.0.1/*"];
await writeFile(path.join(unpacked, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);

const fixture = `<!doctype html><html lang="en"><body><main><p>Visible fixture paragraph</p><label for="editor">Editor</label><textarea id="editor" autofocus>fixture value</textarea><div id="rich" contenteditable="true">rich fixture value</div><input id="secret" type="password" value="synthetic-secret"><iframe id="child" src="/frame"></iframe></main><script>
const editor = document.getElementById("editor");
const rich = document.getElementById("rich");
const secret = document.getElementById("secret");
const child = document.getElementById("child");
const stages = [
  () => editor.focus(),
  () => rich.focus(),
  () => secret.focus(),
  () => {
    child.focus();
    child.contentWindow?.focus();
    child.contentWindow?.postMessage("focus-frame-editor", location.origin);
  },
  () => {
    if (location.pathname !== "/navigated") history.pushState({}, "", "/navigated");
    editor.focus();
  }
];
let stage = 0;
setInterval(() => stages[stage++ % stages.length](), 700);
</script></body></html>`;
const frameFixture = `<!doctype html><html lang="en"><body><p>Iframe fixture paragraph</p><textarea id="frame-editor">iframe fixture value</textarea><script>
addEventListener("message", (event) => {
  if (event.origin === location.origin && event.data === "focus-frame-editor") {
    window.focus();
    document.getElementById("frame-editor").focus();
  }
});
</script></body></html>`;
const http = createServer((request, response) => {
  response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
  response.end(request.url === "/frame" ? frameFixture : fixture);
});
await new Promise((resolve) => http.listen(0, "127.0.0.1", resolve));
const address = http.address();
const fixtureUrl = `http://127.0.0.1:${address.port}/`;
if (!strict) {
  await writeFile(
    bridgePath,
    (await readFile(bridgePath, "utf8"))
      .replaceAll(
        "api.tabs.query({ active: true, lastFocusedWindow: true })",
        `api.tabs.get(globalThis.__lumenTestTabId).then((tab) => [{ ...tab, url: ${JSON.stringify(fixtureUrl)} }])`
      )
      .replace(
        'return errorResult(command, "main_frame_unavailable", "the main frame could not be captured", true);',
        'return errorResult(command, "main_frame_unavailable", `the main frame could not be captured: ${JSON.stringify(frameStatus)}`, true);'
      )
  );
}
const originalWorkerSource = await readFile(workerPath, "utf8");
const workerSource = strict
  ? originalWorkerSource.replace("com.lumen.asr.context_browser", nativeHostName)
  : originalWorkerSource.replace(
    'startNativeBridge("com.lumen.asr.context_browser", "chromium");',
    `async function startTestBridge() {
  const testTab = await chrome.tabs.create({ url: ${JSON.stringify(fixtureUrl)}, active: true });
  globalThis.__lumenTestTabId = testTab.id;
  const navigationDeadline = Date.now() + 10000;
  while (Date.now() < navigationDeadline) {
    const current = await chrome.tabs.get(testTab.id);
    if (current.url || current.pendingUrl) break;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  if (testTab?.windowId) {
    await chrome.windows.update(testTab.windowId, { focused: true }).catch(() => {});
  }
  startNativeBridge("${nativeHostName}", "chromium");
}
void startTestBridge();`
  );
await writeFile(workerPath, workerSource);

const serverBinary = path.join(workspace, "target/debug/lumen-context-browser-e2e-server");
const hostSource = path.join(workspace, "target/debug/lumen-context-browser-host");
const host = path.join(bridgeRoot, "host");
await copyFile(hostSource, host);
await chmod(host, 0o755);
const server = spawn(serverBinary, ["--root", bridgeRoot, "--origin", extensionOrigin], {
  stdio: ["ignore", "pipe", "pipe"]
});
let serverOutput = "";
server.stdout.on("data", (chunk) => { serverOutput += chunk; });
server.stderr.on("data", (chunk) => { serverOutput += chunk; });
const readyDeadline = Date.now() + 10000;
while (!serverOutput.includes("READY")) {
  if (Date.now() >= readyDeadline) throw new Error(`e2e server did not become ready: ${serverOutput}`);
  await new Promise((resolve) => setTimeout(resolve, 25));
}

const nativeHostManifests = [
  path.join(profile, "NativeMessagingHosts", `${nativeHostName}.json`),
  ...[...new Set([os.homedir(), home])].map((root) =>
    path.join(root, `Library/Application Support/${nativeHostProductDirectory}/NativeMessagingHosts/${nativeHostName}.json`)
  )
];
process.on("exit", () => nativeHostManifests.forEach((manifestPath) => rmSync(manifestPath, { force: true })));
const nativeHostContents = `${JSON.stringify({
  name: nativeHostName,
  description: "Lumen browser e2e host",
  path: host,
  type: "stdio",
  allowed_origins: [extensionOrigin]
}, null, 2)}\n`;
for (const manifestPath of nativeHostManifests) {
  await mkdir(path.dirname(manifestPath), { recursive: true });
  await writeFile(manifestPath, nativeHostContents);
}

const browserArguments = [
  "--disable-gpu",
  "--no-first-run",
  "--enable-unsafe-extension-debugging",
  "--remote-debugging-port=0",
  "--enable-logging=stderr",
  "--v=0",
  `--user-data-dir=${profile}`,
  fixtureUrl
];
const headless = process.env.LUMEN_CHROMIUM_HEADLESS === "1"
  || (!strict && process.env.LUMEN_CHROMIUM_HEADLESS !== "0");
if (headless) browserArguments.unshift("--headless=new");
const browser = spawn(chromium, browserArguments, { stdio: ["ignore", "ignore", "pipe"] });
let browserOutput = "";
browser.stderr.on("data", (chunk) => {
  browserOutput = `${browserOutput}${chunk}`.slice(-100000);
});
await loadUnpackedExtension(profile, unpacked, extensionId);

let watchdog;
const exitCode = await Promise.race([
  new Promise((resolve) => server.on("exit", resolve)),
  new Promise((_, reject) => {
    watchdog = setTimeout(
      () => reject(new Error(`browser e2e timed out: ${serverOutput}\n${relevantBrowserOutput(browserOutput)}`)),
      30000
    );
  })
]);
clearTimeout(watchdog);
await stopChild(browser);
http.close();
await Promise.all(nativeHostManifests.map((manifestPath) => rm(manifestPath, { force: true })));
if (exitCode !== 0 || !serverOutput.includes("PASS")) {
  throw new Error(`browser e2e failed: ${serverOutput}\n${relevantBrowserOutput(browserOutput)}`);
}
process.stdout.write(`Chromium browser context ${strict ? "strict" : "protocol"} e2e passed\n`);

async function loadUnpackedExtension(profile, extensionPath, expectedId) {
  const activePort = path.join(profile, "DevToolsActivePort");
  const deadline = Date.now() + 10000;
  let lines;
  while (!lines) {
    try {
      lines = (await readFile(activePort, "utf8")).trim().split("\n");
    } catch {
      if (Date.now() >= deadline) throw new Error("Chrome DevTools endpoint did not start");
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
  }
  const socket = new WebSocket(`ws://127.0.0.1:${lines[0]}${lines[1]}`);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  let nextId = 0;
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++nextId;
    const receive = (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== id) return;
      socket.removeEventListener("message", receive);
      if (message.error) reject(new Error(message.error.message));
      else resolve(message.result);
    };
    socket.addEventListener("message", receive);
    socket.send(JSON.stringify({ id, method, params }));
  });
  const result = await call("Extensions.loadUnpacked", { path: extensionPath });
  if (result.id !== expectedId) {
    throw new Error(`loaded extension id ${result.id} did not match ${expectedId}`);
  }
  socket.close();
}

function relevantBrowserOutput(output) {
  const relevant = output
    .split("\n")
    .filter((line) => /native|extension|manifest|console|error|failed/i.test(line));
  return relevant.slice(-100).join("\n");
}

async function stopChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise((resolve) => child.once("exit", resolve));
  child.kill("SIGTERM");
  const stopped = await Promise.race([
    exited.then(() => true),
    new Promise((resolve) => setTimeout(() => resolve(false), 2000))
  ]);
  if (stopped) return;
  child.kill("SIGKILL");
  await exited;
}
