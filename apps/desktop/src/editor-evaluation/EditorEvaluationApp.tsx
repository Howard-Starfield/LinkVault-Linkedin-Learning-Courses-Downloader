import { lazy, Suspense, useEffect, useRef, useState } from "react";
import type { ClippingNoteEditorHandle } from "../components/newspaper/ClippingNoteEditor";
import {
  CLIPPING_NOTE_ADVERSARIAL_FIXTURE,
  CLIPPING_NOTE_COMMON_FIXTURE,
  CLIPPING_NOTE_EMPTY_FIXTURE,
  CLIPPING_NOTE_HEADINGS_FIXTURE,
  CLIPPING_NOTE_MDX_EDGE_CASES_FIXTURE,
  CLIPPING_NOTE_TWO_MIB_FIXTURE,
  CLIPPING_NOTE_SECOND_DOCUMENT
} from "./fixtures";

const LazyClippingNoteEditor = lazy(async () => {
  const module = await import("../components/newspaper/ClippingNoteEditor");
  return { default: module.ClippingNoteEditor };
});

declare global {
  interface Window {
    __CLIPPING_EDITOR_EVALUATION__?: {
      captureMarkdown: () => string;
      changeCount: number;
      documentId: string;
      readyCount: number;
    };
  }
}

type EvaluationDocument = "fixture" | "second" | "adversarial" | "mdx-edge" | "empty" | "headings" | "two-mib" | "reloaded";

function markdownFor(documentId: EvaluationDocument) {
  if (documentId === "second") return CLIPPING_NOTE_SECOND_DOCUMENT;
  if (documentId === "adversarial") return CLIPPING_NOTE_ADVERSARIAL_FIXTURE;
  if (documentId === "mdx-edge") return CLIPPING_NOTE_MDX_EDGE_CASES_FIXTURE;
  if (documentId === "empty") return CLIPPING_NOTE_EMPTY_FIXTURE;
  if (documentId === "headings") return CLIPPING_NOTE_HEADINGS_FIXTURE;
  if (documentId === "two-mib") return CLIPPING_NOTE_TWO_MIB_FIXTURE;
  return CLIPPING_NOTE_COMMON_FIXTURE;
}

export function EditorEvaluationApp() {
  const editorRef = useRef<ClippingNoteEditorHandle>(null);
  const [documentId, setDocumentId] = useState<EvaluationDocument>("fixture");
  const [markdown, setMarkdown] = useState(CLIPPING_NOTE_COMMON_FIXTURE);
  const [capturedMarkdown, setCapturedMarkdown] = useState("");
  const [changeCount, setChangeCount] = useState(0);
  const [blurCount, setBlurCount] = useState(0);
  const [parentAcknowledgements, setParentAcknowledgements] = useState(0);
  const [noOpParentRerenders, setNoOpParentRerenders] = useState(0);
  const [failedParentAcknowledgements, setFailedParentAcknowledgements] = useState(0);
  const [readyCount, setReadyCount] = useState(0);
  const [readOnly, setReadOnly] = useState(false);
  const [theme, setTheme] = useState<"light" | "dark">("light");

  useEffect(() => {
    window.__CLIPPING_EDITOR_EVALUATION__ = {
      captureMarkdown: () => editorRef.current?.getMarkdown() ?? "",
      changeCount,
      documentId,
      readyCount
    };
    return () => {
      delete window.__CLIPPING_EDITOR_EVALUATION__;
    };
  }, [changeCount, documentId, readyCount]);

  function switchDocument(nextDocumentId: EvaluationDocument) {
    setDocumentId(nextDocumentId);
    setMarkdown(markdownFor(nextDocumentId));
    setCapturedMarkdown("");
  }

  function reloadCapturedMarkdown() {
    const serializedMarkdown = editorRef.current?.getMarkdown() ?? "";
    setDocumentId("reloaded");
    setMarkdown(serializedMarkdown);
    setCapturedMarkdown("");
  }

  return (
    <main className="editor-evaluation" data-theme={theme}>
      <header className="editor-evaluation__header">
        <div>
          <p className="editor-evaluation__eyebrow">Phase 4A non-production harness</p>
          <h1>Clipping note editor compatibility</h1>
        </div>
        <div className="editor-evaluation__actions">
          <button type="button" onClick={() => setTheme((current) => current === "light" ? "dark" : "light")}>
            Toggle {theme === "light" ? "dark" : "light"} theme
          </button>
          <button type="button" onClick={() => setReadOnly((current) => !current)}>
            {readOnly ? "Enable editing" : "Enable read only"}
          </button>
        </div>
      </header>

      <section aria-label="Editor evaluation controls" className="editor-evaluation__controls">
        <button type="button" onClick={() => switchDocument("fixture")}>Load common fixture</button>
        <button type="button" onClick={() => switchDocument("second")}>Switch to second clipping</button>
        <button type="button" onClick={() => switchDocument("empty")}>Load empty document</button>
        <button type="button" onClick={() => switchDocument("headings")}>Load heading fixture</button>
        <button type="button" onClick={() => switchDocument("two-mib")}>Load 2 MiB fixture</button>
        <button type="button" onClick={() => switchDocument("adversarial")}>Load adversarial fixture</button>
        <button type="button" onClick={() => switchDocument("mdx-edge")}>Load MDX edge-case fixture</button>
        <button type="button" onClick={() => setParentAcknowledgements((current) => current + 1)}>
          Simulate parent acknowledgement
        </button>
        <button type="button" onClick={() => setNoOpParentRerenders((current) => current + 1)}>
          Simulate no-op parent rerender
        </button>
        <button type="button" onClick={() => setFailedParentAcknowledgements((current) => current + 1)}>
          Simulate failed parent acknowledgement
        </button>
        <button type="button" onClick={() => setCapturedMarkdown(editorRef.current?.getMarkdown() ?? "")}>
          Capture Markdown
        </button>
        <button type="button" onClick={reloadCapturedMarkdown}>Reload captured Markdown</button>
        <button type="button" onClick={() => editorRef.current?.focus()}>Focus editor</button>
      </section>

      <p aria-live="polite" className="editor-evaluation__status" data-testid="editor-status">
        Document {documentId}; changes {changeCount}; blur events {blurCount}; parent acknowledgements {parentAcknowledgements}; no-op parent rerenders {noOpParentRerenders}; failed parent acknowledgements {failedParentAcknowledgements}; ready events {readyCount}
      </p>

      <Suspense fallback={<p role="status">Loading clipping note editor.</p>}>
        <LazyClippingNoteEditor
          ref={editorRef}
          autoFocus={false}
          documentId={documentId}
          initialMarkdown={markdown}
          onBlur={() => setBlurCount((current) => current + 1)}
          onMarkdownChange={(nextMarkdown) => {
            setMarkdown(nextMarkdown);
            setChangeCount((current) => current + 1);
          }}
          onReady={() => setReadyCount((current) => current + 1)}
          readOnly={readOnly}
        />
      </Suspense>

      <section aria-label="Captured Markdown" className="editor-evaluation__output">
        <h2>Captured Markdown</h2>
        <pre data-testid="captured-markdown">{capturedMarkdown}</pre>
      </section>
    </main>
  );
}
