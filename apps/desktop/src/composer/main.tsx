import React from "react";
import ReactDOM from "react-dom/client";
import ComposerApp from "./ComposerApp";
import "../design/styles.css";
import "../popup/popup.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ComposerApp />
  </React.StrictMode>,
);
