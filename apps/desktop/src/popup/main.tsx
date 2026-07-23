import React from "react";
import ReactDOM from "react-dom/client";
import PopupApp from "./PopupApp";
import "../design/styles.css";
import "./popup.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <PopupApp />
  </React.StrictMode>,
);
