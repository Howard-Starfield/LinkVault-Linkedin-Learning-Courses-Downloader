export type ClippingNoteDocument = {
  documentId: string;
  title: string;
  markdown: string;
  revision: number;
};

export type ClippingNoteSaveStatus = "clean" | "dirty" | "saving" | "failed" | "conflict";

export type ClippingNoteSaveView = {
  documentId: string;
  persistedTitle: string;
  persistedMarkdown: string;
  persistedRevision: number;
  draftTitle: string;
  draftMarkdown: string;
  status: ClippingNoteSaveStatus;
  errorCode: string | null;
};

export type ClippingNoteSave = (request: {
  documentId: string;
  expectedRevision: number;
  title: string;
  markdown: string;
}) => Promise<ClippingNoteDocument>;

type Listener = (view: ClippingNoteSaveView) => void;

export class ClippingNoteSaveController {
  private view: ClippingNoteSaveView;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private inFlight: Promise<boolean> | null = null;
  private listeners = new Set<Listener>();
  private disposed = false;
  private readonly save: ClippingNoteSave;
  private readonly errorCode: (error: unknown) => string;
  private readonly debounceMs: number;
  private readonly validate: (draft: { title: string; markdown: string }) => string | null;

  constructor(
    initial: ClippingNoteDocument,
    save: ClippingNoteSave,
    errorCode: (error: unknown) => string,
    debounceMs = 800,
    validate: (draft: { title: string; markdown: string }) => string | null = () => null
  ) {
    this.save = save;
    this.errorCode = errorCode;
    this.debounceMs = debounceMs;
    this.validate = validate;
    this.view = {
      documentId: initial.documentId,
      persistedTitle: initial.title,
      persistedMarkdown: initial.markdown,
      persistedRevision: initial.revision,
      draftTitle: initial.title,
      draftMarkdown: initial.markdown,
      status: "clean",
      errorCode: null
    };
  }

  getSnapshot = () => this.view;

  subscribe = (listener: Listener) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  setTitle(title: string) {
    this.setDraft(title, this.view.draftMarkdown);
  }

  setMarkdown(markdown: string) {
    this.setDraft(this.view.draftTitle, markdown);
  }

  private setDraft(title: string, markdown: string) {
    if (this.disposed || this.view.status === "conflict") return;
    const clean = title === this.view.persistedTitle && markdown === this.view.persistedMarkdown;
    const validationError = clean ? null : this.validate({ title, markdown });
    this.setView({
      ...this.view,
      draftTitle: title,
      draftMarkdown: markdown,
      status: clean ? "clean" : validationError ? "failed" : this.view.status === "saving" ? "saving" : "dirty",
      errorCode: validationError
    });
    this.clearTimer();
    if (!clean && !validationError && this.view.status !== "saving") {
      this.timer = setTimeout(() => void this.startSave(), this.debounceMs);
    }
  }

  retry() {
    if (this.view.status !== "failed") return Promise.resolve(false);
    return this.startSave();
  }

  async flush() {
    this.clearTimer();
    if (this.view.status === "clean") return true;
    if (this.view.status === "conflict" || this.view.status === "failed") return false;
    if (this.inFlight) {
      const saved = await this.inFlight;
      const afterSave = this.getSnapshot().status as ClippingNoteSaveStatus;
      if (!saved || afterSave === "conflict" || afterSave === "failed") return false;
    }
    if (this.view.status === "dirty") {
      return this.startSave();
    }
    return (this.getSnapshot().status as ClippingNoteSaveStatus) === "clean";
  }

  keepMyChanges(latest: ClippingNoteDocument) {
    if (latest.documentId !== this.view.documentId) return Promise.resolve(false);
    this.adoptPersisted(latest, false);
    this.setView({ ...this.view, status: "dirty", errorCode: null });
    return this.startSave();
  }

  useSavedVersion(latest: ClippingNoteDocument) {
    if (latest.documentId !== this.view.documentId) return;
    this.adoptPersisted(latest, true);
  }

  dispose() {
    this.disposed = true;
    this.clearTimer();
    this.listeners.clear();
  }

  private startSave(): Promise<boolean> {
    if (this.disposed || this.view.status === "conflict") return Promise.resolve(false);
    if (this.inFlight) return this.inFlight;
    if (
      this.view.draftTitle === this.view.persistedTitle &&
      this.view.draftMarkdown === this.view.persistedMarkdown
    ) {
      this.setView({ ...this.view, status: "clean", errorCode: null });
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

    this.clearTimer();
    const submitted = {
      documentId: this.view.documentId,
      expectedRevision: this.view.persistedRevision,
      title: this.view.draftTitle,
      markdown: this.view.draftMarkdown
    };
    this.setView({ ...this.view, status: "saving", errorCode: null });
    const running = this.save(submitted)
      .then((acknowledged) => {
        if (this.disposed || acknowledged.documentId !== this.view.documentId) return false;
        const draftChanged =
          this.view.draftTitle !== submitted.title || this.view.draftMarkdown !== submitted.markdown;
        this.adoptPersisted(acknowledged, !draftChanged);
        if (draftChanged) {
          const clean =
            this.view.draftTitle === acknowledged.title &&
            this.view.draftMarkdown === acknowledged.markdown;
          const queuedValidationError = clean
            ? null
            : this.validate({ title: this.view.draftTitle, markdown: this.view.draftMarkdown });
          this.setView({
            ...this.view,
            status: clean ? "clean" : queuedValidationError ? "failed" : "dirty",
            errorCode: queuedValidationError
          });
        }
        return true;
      })
      .catch((error: unknown) => {
        if (this.disposed) return false;
        const code = this.errorCode(error);
        this.setView({
          ...this.view,
          status: code === "CLIPPING_REVISION_CONFLICT" ? "conflict" : "failed",
          errorCode: code
        });
        return false;
      })
      .finally(() => {
        this.inFlight = null;
        if (!this.disposed && this.view.status === "dirty") {
          this.timer = setTimeout(() => void this.startSave(), this.debounceMs);
        }
      });
    this.inFlight = running;
    return running;
  }

  private adoptPersisted(document: ClippingNoteDocument, replaceDraft: boolean) {
    this.clearTimer();
    this.setView({
      ...this.view,
      persistedTitle: document.title,
      persistedMarkdown: document.markdown,
      persistedRevision: document.revision,
      draftTitle: replaceDraft ? document.title : this.view.draftTitle,
      draftMarkdown: replaceDraft ? document.markdown : this.view.draftMarkdown,
      status: replaceDraft ? "clean" : this.view.status,
      errorCode: null
    });
  }

  private clearTimer() {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }

  private setView(view: ClippingNoteSaveView) {
    this.view = view;
    for (const listener of this.listeners) listener(view);
  }
}
