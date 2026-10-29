import { invoke } from "@tauri-apps/api/core";
import { render } from "solid-js/web";

import App from "./App";
import "./styles.css";

void invoke("log_timing", {
  name: "js_module_start",
  elapsedMs: performance.now(),
  meta: "ui",
}).catch(() => {});

const root = document.getElementById("root");
if (root) {
  // Remove the inline skeleton now that the bundle has parsed; the app renders
  // synchronously from cached state on the next frame.
  root.replaceChildren();
  render(() => <App />, root);
}
