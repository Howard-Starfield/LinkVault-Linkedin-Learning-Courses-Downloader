export function installClippingInstanceActivationBrowserHarness() {
  const handlers = new Set();
  window.__CLIPPING_INSTANCE_ACTIVATION__ = {
    listen(handler) {
      handlers.add(handler);
      return () => handlers.delete(handler);
    },
    async emit() {
      await Promise.all([...handlers].map((handler) => handler()));
    },
    listenerCount() {
      return handlers.size;
    }
  };
}
