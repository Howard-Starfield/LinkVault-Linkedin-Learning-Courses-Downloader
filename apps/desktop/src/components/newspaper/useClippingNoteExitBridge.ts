import { useEffect, type RefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import type { ClippingFlush } from "./NewspaperClippings";
import { ClippingNoteExitPreparation } from "./clipping-note-exit-preparation";

type PrepareRequest = { token: number; reason: "close" | "exit"; deadlineMs: number };
type ExitBridgeHarness = {
  listenPrepare: (handler: (event: { payload: PrepareRequest }) => Promise<void>) => () => void;
  listenBlocked: (handler: (event: { payload: { reason: "close" | "exit" } }) => void) => () => void;
  resolve: (token: number, durable: boolean) => Promise<unknown>;
};

function browserHarness() {
  if (typeof window === "undefined" || window.location.hostname !== "127.0.0.1") return undefined;
  return (window as Window & { __CLIPPING_NOTE_EXIT_BRIDGE__?: ExitBridgeHarness })
    .__CLIPPING_NOTE_EXIT_BRIDGE__;
}

export function useClippingNoteExitBridge(
  enabled: boolean,
  clippingFlushRef: RefObject<ClippingFlush | null>
) {
  useEffect(() => {
    const harness = browserHarness();
    if (!enabled && !harness) return;
    let disposed = false;
    const cleanups: Array<() => void> = [];
    const preparation = new ClippingNoteExitPreparation();
    const retainCleanup = (registration: Promise<() => void>) => {
      void registration.then((cleanup) => {
        if (disposed) cleanup();
        else cleanups.push(cleanup);
      }, () => {
        if (!disposed) toast.error("Exit protection unavailable", {
          description: "Keep LinkVault open while editing clipping notes and retry after restarting the app."
        });
      });
    };

    const prepare = async ({ payload }: { payload: PrepareRequest }) => {
      if (disposed) return;
      const durable = await preparation.prepare(clippingFlushRef.current);
      try {
        if (harness) await harness.resolve(payload.token, durable);
        else await invoke("resolve_cooperative_exit", { token: payload.token, durable });
      } catch {
        if (!disposed) toast.error("Could not confirm note durability", {
          description: "LinkVault will remain open so your clipping draft is not discarded."
        });
      }
    };
    const blocked = ({ payload }: { payload: { reason: "close" | "exit" } }) => {
      if (!disposed) toast.info(payload.reason === "close"
        ? "LinkVault stayed visible to protect an unsaved clipping note."
        : "LinkVault stayed open to protect an unsaved clipping note.");
    };
    retainCleanup(harness
      ? Promise.resolve(harness.listenPrepare(prepare))
      : listen<PrepareRequest>("linkvault://prepare-exit", prepare));
    retainCleanup(harness
      ? Promise.resolve(harness.listenBlocked(blocked))
      : listen<{ reason: "close" | "exit" }>("linkvault://exit-blocked", blocked));

    return () => {
      disposed = true;
      cleanups.forEach((cleanup) => cleanup());
    };
  }, [clippingFlushRef, enabled]);
}
