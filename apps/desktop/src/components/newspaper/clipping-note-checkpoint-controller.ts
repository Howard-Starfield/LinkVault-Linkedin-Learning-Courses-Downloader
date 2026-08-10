export type ClippingNoteCheckpointStatus =
  | "idle"
  | "dirty"
  | "checkpointing"
  | "durable"
  | "failed"
  | "conflict";

export type ClippingNoteCheckpointView = {
  documentId: string;
  writerSessionId: string;
  writerSequence: number;
  durableSequence: number;
  baseRevision: number;
  draftTitle: string;
  draftMarkdown: string;
  status: ClippingNoteCheckpointStatus;
  errorCode: string | null;
};

export type ClippingNoteCheckpointRequest = {
  documentId: string;
  writerSessionId: string;
  writerSequence: number;
  baseRevision: number;
  title: string;
  markdown: string;
};

export type ClippingNoteCheckpointAck = Pick<
  ClippingNoteCheckpointRequest,
  "documentId" | "writerSessionId" | "writerSequence"
>;

export type ClippingNoteCheckpointWrite = (
  request: ClippingNoteCheckpointRequest
) => Promise<ClippingNoteCheckpointAck>;

export type ClippingNoteCheckpointTimerScheduler = {
  schedule: (callback: () => void, delayMs: number) => unknown;
  cancel: (handle: unknown) => void;
};

type Listener = (view: ClippingNoteCheckpointView) => void;

const systemTimers: ClippingNoteCheckpointTimerScheduler = {
  schedule: (callback, delayMs) => setTimeout(callback, delayMs),
  cancel: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>)
};

export class ClippingNoteCheckpointController {
  private view: ClippingNoteCheckpointView;
  private debounceTimer: unknown | null = null;
  private maxWaitTimer: unknown | null = null;
  private inFlight: Promise<boolean> | null = null;
  private listeners = new Set<Listener>();
  private disposed = false;
  private readonly write: ClippingNoteCheckpointWrite;
  private readonly errorCode: (error: unknown) => string;
  private readonly debounceMs: number;
  private readonly maxWaitMs: number;
  private readonly validate: (draft: { title: string; markdown: string }) => string | null;
  private readonly timers: ClippingNoteCheckpointTimerScheduler;

  constructor(
    documentId: string,
    writerSessionId: string,
    baseRevision: number,
    initialTitle: string,
    initialMarkdown: string,
    write: ClippingNoteCheckpointWrite,
    errorCode: (error: unknown) => string,
    debounceMs = 500,
    maxWaitMs = 2_000,
    validate: (draft: { title: string; markdown: string }) => string | null = () => null,
    timers: ClippingNoteCheckpointTimerScheduler = systemTimers
  ) {
    this.write = write;
    this.errorCode = errorCode;
    this.debounceMs = debounceMs;
    this.maxWaitMs = maxWaitMs;
    this.validate = validate;
    this.timers = timers;
    this.view = {
      documentId,
      writerSessionId,
      writerSequence: 0,
      durableSequence: 0,
      baseRevision,
      draftTitle: initialTitle,
      draftMarkdown: initialMarkdown,
      status: "idle",
      errorCode: null
    };
  }

  getSnapshot = () => this.view;
  subscribe = (listener: Listener) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
  setDraft(baseRevision: number, title: string, markdown: string) {
    if (this.disposed) return;
    if (
      baseRevision === this.view.baseRevision
      && title === this.view.draftTitle
      && markdown === this.view.draftMarkdown
    ) return;

    const validationError = this.validate({ title, markdown });
    const preserveConflict = this.view.status === "conflict";
    this.setView({
      ...this.view,
      baseRevision,
      draftTitle: title,
      draftMarkdown: markdown,
      writerSequence: this.view.writerSequence + 1,
      status: preserveConflict ? "conflict" : validationError ? "failed" : "dirty",
      errorCode: preserveConflict ? this.view.errorCode : validationError
    });
    this.clearDebounceTimer();
    if (preserveConflict || validationError) {
      this.clearMaxWaitTimer();
    } else {
      this.scheduleCheckpoint();
    }
  }
  retry() {
    if (this.view.status !== "failed") return Promise.resolve(false);
    return this.startCheckpoint();
  }
  async ensureDurable(writerSequence?: number) {
    if (writerSequence !== undefined && (writerSequence < 0 || writerSequence > this.view.writerSequence)) return false;
    while (!this.disposed) {
      this.clearTimers();
      const targetSequence = writerSequence ?? this.view.writerSequence;
      if (targetSequence <= this.view.durableSequence) return true;
      if (this.view.status === "conflict" || this.view.status === "failed") return false;
      const running = this.inFlight
        ?? (this.view.status === "dirty" ? this.startCheckpoint() : null);
      if (!running || !(await running)) return false;
    }
    return false;
  }
  restoreDurable(baseRevision: number, title: string, markdown: string, writerSequence: number) {
    if (this.disposed || this.view.writerSequence !== 0 || writerSequence < 1) return false;
    this.clearTimers();
    this.setView({
      ...this.view,
      baseRevision,
      draftTitle: title,
      draftMarkdown: markdown,
      writerSequence,
      durableSequence: writerSequence,
      status: "durable",
      errorCode: null
    });
    return true;
  }
  resetCanonical(baseRevision: number, title: string, markdown: string) {
    if (this.disposed) return;
    this.clearTimers();
    this.setView({
      ...this.view,
      baseRevision,
      draftTitle: title,
      draftMarkdown: markdown,
      durableSequence: this.view.writerSequence,
      status: "idle",
      errorCode: null
    });
  }
  acknowledgeCanonicalSave(writerSessionId: string, writerSequence: number, canonicalRevision: number) {
    if (this.disposed || writerSessionId !== this.view.writerSessionId) return false;
    if (writerSequence < 0 || writerSequence > this.view.writerSequence) return false;
    const latestIsCanonical = writerSequence === this.view.writerSequence;
    this.clearTimers();
    this.setView({
      ...this.view,
      baseRevision: canonicalRevision,
      durableSequence: Math.max(this.view.durableSequence, writerSequence),
      status: latestIsCanonical ? "idle" : "dirty",
      errorCode: null
    });
    if (!latestIsCanonical) this.scheduleCheckpoint();
    return true;
  }
  dispose() {
    this.disposed = true;
    this.clearTimers();
    this.listeners.clear();
  }

  private startCheckpoint(): Promise<boolean> {
    if (this.disposed || this.view.status === "conflict") return Promise.resolve(false);
    if (this.inFlight) return this.inFlight;
    if (this.view.writerSequence <= this.view.durableSequence) {
      this.setView({ ...this.view, status: "durable", errorCode: null });
      return Promise.resolve(true);
    }
    const validationError = this.validate({
      title: this.view.draftTitle,
      markdown: this.view.draftMarkdown
    });
    if (validationError) {
      this.setView({ ...this.view, status: "failed", errorCode: validationError });
      return Promise.resolve(false);
    }

    this.clearTimers();
    const submitted: ClippingNoteCheckpointRequest = {
      documentId: this.view.documentId,
      writerSessionId: this.view.writerSessionId,
      writerSequence: this.view.writerSequence,
      baseRevision: this.view.baseRevision,
      title: this.view.draftTitle,
      markdown: this.view.draftMarkdown
    };
    this.setView({ ...this.view, status: "checkpointing", errorCode: null });
    const running = this.write(submitted)
      .then((acknowledged) => {
        if (this.disposed) return false;
        if (
          acknowledged.documentId !== submitted.documentId
          || acknowledged.writerSessionId !== submitted.writerSessionId
          || acknowledged.writerSequence !== submitted.writerSequence
        ) {
          this.setView({ ...this.view, status: "failed", errorCode: "CLIPPING_RECOVERY_STALE_ACK" });
          return false;
        }
        const latest = this.view.writerSequence === acknowledged.writerSequence;
        this.setView({
          ...this.view,
          durableSequence: Math.max(this.view.durableSequence, acknowledged.writerSequence),
          status: latest ? "durable" : "dirty",
          errorCode: null
        });
        return true;
      })
      .catch((error: unknown) => {
        if (this.disposed) return false;
        const code = this.errorCode(error);
        this.setView({
          ...this.view,
          status: code === "CLIPPING_RECOVERY_WRITER_CONFLICT" ? "conflict" : "failed",
          errorCode: code
        });
        return false;
      })
      .finally(() => {
        this.inFlight = null;
        if (!this.disposed && this.view.status === "dirty") this.scheduleCheckpoint();
      });
    this.inFlight = running;
    return running;
  }

  private scheduleCheckpoint() {
    this.clearDebounceTimer();
    this.debounceTimer = this.timers.schedule(() => {
      this.debounceTimer = null;
      void this.startCheckpoint();
    }, this.debounceMs);
    if (this.maxWaitTimer === null) {
      this.maxWaitTimer = this.timers.schedule(() => {
        this.maxWaitTimer = null;
        this.clearDebounceTimer();
        void this.startCheckpoint();
      }, this.maxWaitMs);
    }
  }

  private clearDebounceTimer() {
    if (this.debounceTimer === null) return;
    this.timers.cancel(this.debounceTimer);
    this.debounceTimer = null;
  }

  private clearMaxWaitTimer() {
    if (this.maxWaitTimer === null) return;
    this.timers.cancel(this.maxWaitTimer);
    this.maxWaitTimer = null;
  }

  private clearTimers() {
    this.clearDebounceTimer();
    this.clearMaxWaitTimer();
  }

  private setView(view: ClippingNoteCheckpointView) {
    this.view = view;
    for (const listener of this.listeners) listener(view);
  }
}
