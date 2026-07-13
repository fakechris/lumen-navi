# Browser Context Bridge

This extension captures bounded, user-approved browser context for local Lumen consumers. It is
transport only: prompt construction, model inference, rewriting, and personalization are outside
this package.

## Chromium and Edge

The Navi and ASR manifests are independent MV3 packages. Each uses an exact extension origin,
owner, socket, token, and native-host config. Site access is optional and granted per origin.

```sh
npm run package:navi
npm run package:asr
npm run test:e2e:chromium
```

The default Chrome e2e uses an isolated profile and fixed local fixture tab so it can run in a
locked automation session. Run the strict foreground-tab selection after unlocking the console:

```sh
npm run test:e2e:chromium:strict
```

## Safari

Safari does not support the MV3 `background.type` field. Packaging therefore emits one classic
service worker from the shared extractor and bridge sources. Commands travel from the containing
macOS app through `SFSafariApplication.dispatchMessage`; results return through
`runtime.sendNativeMessage` to the native extension handler.

ASR and Navi wrappers are generated separately and do not depend on each other:

```sh
npm run package:safari:macos:asr
npm run package:safari:macos:navi
```

Both generated wrappers contain an app-side authenticated Unix-socket relay and a native extension
handler. They use separate application groups:

- ASR: `group.org.lumen.asr.context`
- Navi: `group.org.lumen.navi.context`

For a signed live build, the corresponding host application must have the same application-group
entitlement and place `browser-host.json`, `bridge.sock`, and `bridge.token` in that group
container. Unsigned builds verify converter, Swift, resources, entitlements, containing app, and
embedded extension compilation.

Development builds may be signed with a local codesigning certificate; no Apple Team is required
for local relay and App Group testing:

```sh
LUMEN_SAFARI_CODESIGN_IDENTITY="Lumen Local Codesign" npm run package:safari:macos:asr
LUMEN_SAFARI_CODESIGN_IDENTITY="Lumen Local Codesign" npm run package:safari:macos:navi
```

The build signs nested Swift libraries, the embedded extension with its entitlements, and the
containing app with its entitlements, then runs `codesign --verify --deep --strict`. Each generated
app also supports redacted development probes:

```sh
".../Lumen ASR Context Bridge.app/Contents/MacOS/Lumen ASR Context Bridge" --probe-app-group
".../Lumen ASR Context Bridge.app/Contents/MacOS/Lumen ASR Context Bridge" --probe-extension-state
```

Safari enablement remains an explicit user action. A trusted Apple Team identity and a repeat of
the signing/runtime checks remain release gates.

The Rust consumers expose the group-container location without hard-coding a Team ID. After
signing, configure the resolved container path and include the matching Safari relay origin:

```toml
# Navi navi.toml
[browser]
bridge_dir = "/Users/me/Library/Group Containers/TEAMID.group.org.lumen.navi.context"
extension_origins = ["safari-web-extension://org.lumen.navi.context/"]

# ASR settings.toml
[context]
browser_bridge_dir = "/Users/me/Library/Group Containers/TEAMID.group.org.lumen.asr.context"
browser_extension_origins = ["safari-web-extension://org.lumen.asr.context/"]
```

When unset, both consumers retain their existing `data_dir/context-browser` behavior.

## Verification

```sh
npm test
npm run check
```

Tests cover bounded extraction, password redaction, shadow DOM, domain denial, empty-frame fallback,
iframe focus selection, manifest permissions, native-host origin pinning, and Safari classic-worker
packaging. The strict Chrome e2e matrix covers textarea, contenteditable, password redaction,
same-origin iframe focus, all-frame collection, and same-document navigation.
