Underline tab bar — the family's section switcher (transcript editor tabs, source-detail memory/original/claims). Controlled.

```jsx
const [tab, setTab] = React.useState("transcript");
<Tabs active={tab} onChange={setTab}
  tabs={[{id:"transcript",label:"Transcript"},{id:"speakers",label:"Speakers"},{id:"timeline",label:"Timeline"}]} />
```
