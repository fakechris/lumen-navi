Dashboard metric tile — big tabular figure, uppercase label, optional hint. Used for the "Today" counters (came in / read / crystallized / attention).

```jsx
<StatCard label="Came in" value={3} hint="today" />
<StatCard label="Attention" value={1} tone="warn" hint="blocked source" onClick={goAttention} />
```
