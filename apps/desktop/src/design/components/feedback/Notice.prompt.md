Inline status banner in four semantic tones (info/success/warn/danger). Quiet tinted fill with matching icon + text; only `warn` gets a border.

```jsx
<Notice tone="success">Transcript exported to SRT.</Notice>
<Notice tone="warn" title="Accessibility not granted">Lumen will copy to the clipboard instead of pasting.</Notice>
<Notice tone="danger" title="Transcription failed">Model files not found in the shared cluster path.</Notice>
```
