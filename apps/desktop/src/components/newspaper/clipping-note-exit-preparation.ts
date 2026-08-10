export class ClippingNoteExitPreparation {
  private inFlight: Promise<boolean> | null = null;

  prepare(flush: (() => Promise<boolean>) | null) {
    if (!flush) return Promise.resolve(true);
    if (this.inFlight) return this.inFlight;
    const running = Promise.resolve()
      .then(flush)
      .catch(() => false)
      .finally(() => {
        if (this.inFlight === running) this.inFlight = null;
      });
    this.inFlight = running;
    return running;
  }
}
