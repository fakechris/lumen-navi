Slim progress bar for transcription, translation, and model downloads. Pass `value` 0–100 (with optional `label`); omit `value` for an indeterminate track.

```jsx
<ProgressBar value={62} label="Transcribing" />
<ProgressBar value={100} tone="success" label="Complete" />
<ProgressBar label="Preparing model…" />
```
