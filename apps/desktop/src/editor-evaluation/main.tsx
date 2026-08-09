import React from "react";
import ReactDOM from "react-dom/client";
import "../index.css";
import "./editor-evaluation.css";
import { EditorEvaluationApp } from "./EditorEvaluationApp";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <EditorEvaluationApp />
  </React.StrictMode>
);
