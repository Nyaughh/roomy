import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/instrument-sans/standard.css";
import "./tokens.css";
import "./styles.css";
import { App } from "./App";
import { initTheme } from "./theme";

initTheme();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
