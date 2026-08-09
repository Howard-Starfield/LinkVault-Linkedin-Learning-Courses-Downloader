import { EditorContent, useEditor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { Markdown } from "@tiptap/markdown";
import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState
} from "react";
import {
  isSafeClippingNoteLink,
  normalizeClippingNoteMarkdown
} from "./clipping-note-editor-markdown";

export type ClippingNoteEditorProps = {
  documentId: string;
  initialMarkdown: string;
  readOnly?: boolean;
  autoFocus?: boolean;
  onMarkdownChange: (markdown: string) => void;
  onBlur: () => void;
  onReady?: () => void;
};

export type ClippingNoteEditorHandle = {
  focus: () => void;
  getMarkdown: () => string;
};

type HeadingLevel = 0 | 1 | 2 | 3 | 4;

function selectedHeadingLevel(editor: NonNullable<ReturnType<typeof useEditor>>): HeadingLevel {
  for (const level of [1, 2, 3, 4] as const) {
    if (editor.isActive("heading", { level })) return level;
  }
  return 0;
}

/**
 * Phase 4A's retained candidate: Tiptap 3.29.2 constrained to the V1
 * Markdown subset. It deliberately has no Tauri or autosave dependency.
 */
export const ClippingNoteEditor = forwardRef<ClippingNoteEditorHandle, ClippingNoteEditorProps>(
  function ClippingNoteEditor(
    { documentId, initialMarkdown, readOnly = false, autoFocus = false, onMarkdownChange, onBlur, onReady },
    forwardedRef
  ) {
    const normalizedInitialMarkdown = useMemo(
      () => normalizeClippingNoteMarkdown(initialMarkdown),
      [documentId]
    );
    const activeDocumentIdRef = useRef(documentId);
    const composingRef = useRef(false);
    const pendingCompositionMarkdownRef = useRef<{ documentId: string; markdown: string } | null>(null);
    const lastEmittedMarkdownRef = useRef(normalizedInitialMarkdown);
    const onMarkdownChangeRef = useRef(onMarkdownChange);
    const onBlurRef = useRef(onBlur);
    const onReadyRef = useRef(onReady);
    const linkButtonRef = useRef<HTMLButtonElement | null>(null);
    const linkInputRef = useRef<HTMLInputElement | null>(null);
    const [linkDialogOpen, setLinkDialogOpen] = useState(false);
    const [linkValue, setLinkValue] = useState("");
    const [linkError, setLinkError] = useState("");
    const [pasteNotice, setPasteNotice] = useState("");
    const [transactionRevision, setTransactionRevision] = useState(0);

    useEffect(() => {
      onMarkdownChangeRef.current = onMarkdownChange;
      onBlurRef.current = onBlur;
      onReadyRef.current = onReady;
    }, [onBlur, onMarkdownChange, onReady]);

    useEffect(() => {
      setLinkDialogOpen(false);
      setLinkError("");
    }, [documentId]);

    // Reset wrapper-owned composition state during render as well as through
    // the keyed Tiptap instance. An old composition event must never flush
    // into a newly selected clipping between commit and effect cleanup.
    if (activeDocumentIdRef.current !== documentId) {
      activeDocumentIdRef.current = documentId;
      composingRef.current = false;
      pendingCompositionMarkdownRef.current = null;
      lastEmittedMarkdownRef.current = normalizedInitialMarkdown;
    }

    const emitMarkdownOnce = useCallback((eventDocumentId: string, markdown: string) => {
      if (eventDocumentId !== activeDocumentIdRef.current) return;
      if (lastEmittedMarkdownRef.current === markdown) return;
      lastEmittedMarkdownRef.current = markdown;
      onMarkdownChangeRef.current(markdown);
    }, []);

    const editor = useEditor({
      extensions: [
        StarterKit.configure({
          heading: { levels: [1, 2, 3, 4] },
          code: false,
          codeBlock: false,
          dropcursor: false,
          gapcursor: false,
          horizontalRule: false,
          trailingNode: false,
          underline: false,
          link: {
            autolink: false,
            linkOnPaste: false,
            markdownLinks: true,
            openOnClick: false,
            protocols: ["http", "https", "mailto"],
            isAllowedUri: (url) => isSafeClippingNoteLink(url)
          }
        }),
        Markdown.configure({
          indentation: { style: "space", size: 2 },
          markedOptions: { breaks: false, gfm: true }
        })
      ],
      content: normalizedInitialMarkdown,
      contentType: "markdown",
      editable: !readOnly,
      // Defer construction until the committed effect phase. This prevents
      // React Strict Mode's development-only discarded render from creating a
      // ghost editor instance and duplicate ready notifications.
      immediatelyRender: false,
      shouldRerenderOnTransaction: false,
      editorProps: {
        attributes: {
          "aria-label": "Clipping note editor body",
          "aria-multiline": "true",
          class: "clipping-note-editor__content",
          role: "textbox"
        }
      },
      onBlur: () => onBlurRef.current(),
      onCreate: () => {
        queueMicrotask(() => onReadyRef.current?.());
      },
      onUpdate: ({ editor: updatedEditor }) => {
        const markdown = normalizeClippingNoteMarkdown(updatedEditor.getMarkdown());
        if (composingRef.current) {
          pendingCompositionMarkdownRef.current = { documentId, markdown };
          return;
        }
        emitMarkdownOnce(documentId, markdown);
      }
    }, [documentId]);

    useEffect(() => {
      if (!editor) return;
      editor.setEditable(!readOnly);
    }, [editor, readOnly]);

    useEffect(() => {
      if (!editor || !autoFocus) return;
      const frame = requestAnimationFrame(() => editor.commands.focus("end"));
      return () => cancelAnimationFrame(frame);
    }, [autoFocus, editor]);

    useEffect(() => {
      if (!editor) return;
      const handleTransaction = () => setTransactionRevision((current) => current + 1);
      editor.on("transaction", handleTransaction);
      return () => {
        editor.off("transaction", handleTransaction);
      };
    }, [editor]);

    useEffect(() => {
      if (!linkDialogOpen) return;
      const frame = requestAnimationFrame(() => linkInputRef.current?.focus());
      return () => cancelAnimationFrame(frame);
    }, [linkDialogOpen]);

    useImperativeHandle(
      forwardedRef,
      () => ({
        focus: () => editor?.commands.focus("end"),
        getMarkdown: () => normalizeClippingNoteMarkdown(editor?.getMarkdown() ?? normalizedInitialMarkdown)
      }),
      [editor, normalizedInitialMarkdown]
    );

    const flushCompletedComposition = useCallback((eventDocumentId: string) => {
      if (eventDocumentId !== activeDocumentIdRef.current) return;
      composingRef.current = false;
      queueMicrotask(() => {
        if (eventDocumentId !== activeDocumentIdRef.current) return;
        const markdown = editor
          ? normalizeClippingNoteMarkdown(editor.getMarkdown())
          : (pendingCompositionMarkdownRef.current?.documentId === eventDocumentId
            ? pendingCompositionMarkdownRef.current.markdown
            : null);
        pendingCompositionMarkdownRef.current = null;
        if (markdown !== null && markdown !== undefined) {
          emitMarkdownOnce(eventDocumentId, markdown);
        }
      });
    }, [editor, emitMarkdownOnce]);

    const closeLinkDialog = useCallback(() => {
      setLinkDialogOpen(false);
      setLinkError("");
      requestAnimationFrame(() => linkButtonRef.current?.focus());
    }, []);

    const submitLink = useCallback(() => {
      const url = linkValue.trim();
      if (!editor || !isSafeClippingNoteLink(url)) {
        setLinkError("Use an http, https, or mailto link.");
        return;
      }
      editor.chain().focus().extendMarkRange("link").setLink({ href: url }).run();
      closeLinkDialog();
    }, [closeLinkDialog, editor, linkValue]);

    const handlePasteCapture = useCallback((event: React.ClipboardEvent<HTMLDivElement>) => {
      const transfer = event.clipboardData;
      const containsFile = Array.from(transfer.items).some((item) => item.kind === "file") || transfer.files.length > 0;
      if (containsFile) {
        event.preventDefault();
        setPasteNotice("Images aren't supported inside clipping notes.");
        return;
      }

      // Rich HTML is deliberately flattened to plain text before it reaches
      // ProseMirror, so arbitrary pasted markup cannot create a node/mark that
      // lies outside the Markdown subset.
      if (transfer.types.includes("text/html")) {
        event.preventDefault();
        const plainText = transfer.getData("text/plain");
        if (plainText && editor) {
          editor.view.dispatch(editor.state.tr.insertText(plainText));
        }
      }
    }, [editor]);

    const handleLinkDialogKeyDown = useCallback((event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeLinkDialog();
        return;
      }
      if (event.key !== "Tab") return;
      const dialog = event.currentTarget;
      const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(
        'input:not([disabled]), button:not([disabled])'
      ));
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }, [closeLinkDialog]);

    const controlsDisabled = !editor || readOnly;
    const headingLevel = editor ? selectedHeadingLevel(editor) : 0;

    return (
      <div
        aria-label="Clipping note editor"
        className="clipping-note-editor"
        data-editor-root="true"
        data-editor-transaction={transactionRevision}
        onCompositionEndCapture={() => flushCompletedComposition(documentId)}
        onCompositionStartCapture={() => {
          composingRef.current = true;
        }}
        onPasteCapture={handlePasteCapture}
      >
        <div aria-label="Clipping note formatting" className="clipping-note-editor__toolbar" role="toolbar">
          <button
            aria-label="Undo"
            disabled={controlsDisabled || !editor.can().undo()}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => editor?.chain().focus().undo().run()}
            title="Undo"
            type="button"
          >
            Undo
          </button>
          <button
            aria-label="Redo"
            disabled={controlsDisabled || !editor.can().redo()}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => editor?.chain().focus().redo().run()}
            title="Redo"
            type="button"
          >
            Redo
          </button>
          <label className="clipping-note-editor__heading-label">
            <span className="sr-only">Heading</span>
            <select
              aria-label="Heading"
              disabled={controlsDisabled}
              onChange={(event) => {
                const level = Number(event.target.value) as HeadingLevel;
                if (!editor) return;
                if (level === 0) editor.chain().focus().setParagraph().run();
                else editor.chain().focus().toggleHeading({ level }).run();
              }}
              value={headingLevel}
            >
              <option value={0}>Paragraph</option>
              <option value={1}>Heading 1</option>
              <option value={2}>Heading 2</option>
              <option value={3}>Heading 3</option>
              <option value={4}>Heading 4</option>
            </select>
          </label>
          <ToolbarButton active={editor?.isActive("bold") ?? false} disabled={controlsDisabled} label="Bold" onClick={() => editor?.chain().focus().toggleBold().run()} />
          <ToolbarButton active={editor?.isActive("italic") ?? false} disabled={controlsDisabled} label="Italic" onClick={() => editor?.chain().focus().toggleItalic().run()} />
          <ToolbarButton active={editor?.isActive("strike") ?? false} disabled={controlsDisabled} label="Strikethrough" onClick={() => editor?.chain().focus().toggleStrike().run()} />
          <ToolbarButton active={editor?.isActive("bulletList") ?? false} disabled={controlsDisabled} label="Bulleted list" onClick={() => editor?.chain().focus().toggleBulletList().run()} />
          <ToolbarButton active={editor?.isActive("orderedList") ?? false} disabled={controlsDisabled} label="Numbered list" onClick={() => editor?.chain().focus().toggleOrderedList().run()} />
          <ToolbarButton active={editor?.isActive("blockquote") ?? false} disabled={controlsDisabled} label="Blockquote" onClick={() => editor?.chain().focus().toggleBlockquote().run()} />
          <button
            aria-expanded={linkDialogOpen}
            aria-label="Link"
            disabled={controlsDisabled}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => {
              setLinkError("");
              setLinkValue(editor?.getAttributes("link").href ?? "");
              setLinkDialogOpen(true);
            }}
            ref={linkButtonRef}
            title="Link"
            type="button"
          >
            Link
          </button>
        </div>

        {linkDialogOpen && (
          <div
            aria-label="Insert link"
            aria-modal="true"
            className="clipping-note-editor__link-dialog"
            onKeyDown={handleLinkDialogKeyDown}
            role="dialog"
          >
            <label>
              Link address
              <input
                aria-describedby={linkError ? "clipping-note-editor-link-error" : undefined}
                onChange={(event) => setLinkValue(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    submitLink();
                  }
                }}
                ref={linkInputRef}
                type="url"
                value={linkValue}
              />
            </label>
            {linkError && <p id="clipping-note-editor-link-error" role="alert">{linkError}</p>}
            <div className="clipping-note-editor__link-actions">
              <button onClick={submitLink} type="button">Apply link</button>
              <button onClick={closeLinkDialog} type="button">Cancel</button>
            </div>
          </div>
        )}

        {pasteNotice && <p className="clipping-note-editor__notice" role="status">{pasteNotice}</p>}
        <EditorContent editor={editor} />
      </div>
    );
  }
);

type ToolbarButtonProps = {
  active: boolean;
  disabled: boolean;
  label: string;
  onClick: () => void;
};

function ToolbarButton({ active, disabled, label, onClick }: ToolbarButtonProps) {
  return (
    <button
      aria-label={label}
      aria-pressed={active}
      disabled={disabled}
      onMouseDown={(event) => event.preventDefault()}
      onClick={onClick}
      title={label}
      type="button"
    >
      {label}
    </button>
  );
}
