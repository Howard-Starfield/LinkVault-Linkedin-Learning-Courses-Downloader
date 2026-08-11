import { listen } from "@tauri-apps/api/event";

type ActivationHarness = {
  listen: (handler: () => void | Promise<void>) => () => void;
};

function browserHarness() {
  if (typeof window === "undefined" || window.location.hostname !== "127.0.0.1") return undefined;
  return (window as Window & { __CLIPPING_INSTANCE_ACTIVATION__?: ActivationHarness })
    .__CLIPPING_INSTANCE_ACTIVATION__;
}

export function canListenForClippingInstanceActivation() {
  return Boolean(browserHarness())
    || (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window);
}

export function listenForClippingInstanceActivation(handler: () => void | Promise<void>) {
  const harness = browserHarness();
  if (harness) return Promise.resolve(harness.listen(handler));
  return listen("linkvault://instance-activated", () => { void handler(); });
}
