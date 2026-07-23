Small status dot for live/inline state. `running` pulses; `done`/`failed`/`idle` are static. Optional trailing label.

```jsx
<StatusDot status="running" label="Transcribing" />
<StatusDot status="done" label="Ready" />
<StatusDot status="failed" label="Error" />
```
