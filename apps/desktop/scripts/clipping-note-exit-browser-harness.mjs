export function installClippingNoteExitBrowserHarness() {
  const prepareListeners = new Set();
  const blockedListeners = new Set();
  window.__CLIPPING_NOTE_EXIT_BRIDGE__ = {
    resolutions: [],
    listenPrepare(handler) {
      prepareListeners.add(handler);
      return () => prepareListeners.delete(handler);
    },
    listenBlocked(handler) {
      blockedListeners.add(handler);
      return () => blockedListeners.delete(handler);
    },
    async resolve(token, durable) {
      this.resolutions.push({ token, durable });
      return true;
    },
    emitPrepare(payload) {
      return Promise.all([...prepareListeners].map((handler) => handler({ payload })));
    },
    listenerCounts() {
      return { prepare: prepareListeners.size, blocked: blockedListeners.size };
    }
  };
}
