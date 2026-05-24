import React from "react";
import ReactDOM from "react-dom/client";
import { Toaster } from "sonner";
import App from "./App";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
    <Toaster
      theme="dark"
      visibleToasts={5}
      position="bottom-right"
      className="lv-toaster"
      toastOptions={{
        classNames: {
          toast: "lv-toast",
          title: "lv-toast-title",
          description: "lv-toast-description",
          success: "lv-toast-success",
          error: "lv-toast-error",
          warning: "lv-toast-warning",
          info: "lv-toast-info"
        }
      }}
    />
  </React.StrictMode>
);
