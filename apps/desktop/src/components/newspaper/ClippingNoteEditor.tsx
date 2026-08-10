import { type Editor } from "@tiptap/core";
import { TaskItem, TaskList } from "@tiptap/extension-list";
import { Markdown } from "@tiptap/markdown";
import { EditorContent, useEditor } from "@tiptap/react";
import { BubbleMenu } from "@tiptap/react/menus";
import StarterKit from "@tiptap/starter-kit";
import {
  Bold,
  Italic,
  Link2,
  Redo2,
  Strikethrough,
  Undo2,
  type LucideIcon
} from "lucide-react";
import {
  forwardRef,
  type ReactNode,
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
import { createClippingNoteSlashCommandExtension } from "./clipping-note-slash-command";

export type ClippingNoteEditorProps = {
  documentId: string;
  initialMarkdown: string;
  readOnly?: boolean;
  autoFocus?: boolean;
  onMarkdownChange: (markdown: string) => void;
  onBlur: () => void;
  onReady?: () => void;
  footerContent?: ReactNode;
};

export type ClippingNoteEditorHandle = {
  focus: () => void;
  getMarkdown: () => string;
};

function selectionStartVirtualElement(editor: Editor) {
  const { from, to } = editor.state.selection;
  if (from === to) return null;
  const coordinates = editor.view.coordsAtPos(from);
  const rect = new DOMRect(coordinates.left, coordinates.top, 1, coordinates.bottom - coordinates.top);
  return {
    contextElement: editor.view.dom,
    getBoundingClientRect: () => rect,
    getClientRects: () => [rect]
  };
}

/**
 * Tiptap 3.29.2 constrained to the V1 Markdown subset. The adapter owns all
 * editor menus and deliberately has no Tauri or autosave dependency.
 */
export const ClippingNoteEditor = forwardRef<ClippingNoteEditorHandle, ClippingNoteEditorProps>(
  function ClippingNoteEditor(
    { documentId, initialMarkdown, readOnly = false, autoFocus = false, onMarkdownChange, onBlur, onReady, footerContent },
    forwardedRef
  ) {
    const normalizedInitialMarkdown = useMemo(
      () => normalizeClippingNoteMarkdown(initialMarkdown),
      [documentId]
    );
    const slashCommandExtension = useMemo(() => createClippingNoteSlashCommandExtension(), [documentId]);
    const activeDocumentIdRef = useRef(documentId);
    const composingRef = useRef(false);
    const selectionMenuArmedRef = useRef(false);
    const pendingCompositionMarkdownRef = useRef<{ documentId: string; markdown: string } | null>(null);
    const lastEmittedMarkdownRef = useRef(normalizedInitialMarkdown);
    const onMarkdownChangeRef = useRef(onMarkdownChange);
    const onBlurRef = useRef(onBlur);
    const onReadyRef = useRef(onReady);
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
      selectionMenuArmedRef.current = false;
    }, [documentId]);

    if (activeDocumentIdRef.current !== documentId) {
      activeDocumentIdRef.current = documentId;
      composingRef.current = false;
      selectionMenuArmedRef.current = false;
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
        TaskList,
        TaskItem.configure({ nested: true }),
        Markdown.configure({
          indentation: { style: "space", size: 2 },
          markedOptions: { breaks: false, gfm: true }
        }),
        slashCommandExtension
      ],
      content: normalizedInitialMarkdown,
      contentType: "markdown",
      editable: !readOnly,
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
      onCreate: () => queueMicrotask(() => onReadyRef.current?.()),
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
      const frame = requestAnimationFrame(() => { if (document.activeElement === document.body) editor.commands.focus("end"); });
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
        if (markdown !== null && markdown !== undefined) emitMarkdownOnce(eventDocumentId, markdown);
      });
    }, [editor, emitMarkdownOnce]);

    const closeLinkDialog = useCallback(() => {
      setLinkDialogOpen(false);
      setLinkError("");
      requestAnimationFrame(() => editor?.commands.focus());
    }, [editor]);

    const submitLink = useCallback(() => {
      const url = linkValue.trim();
      if (!editor || !isSafeClippingNoteLink(url)) {
        setLinkError("Use an http, https, or mailto link.");
        return;
      }
      editor.chain().focus().extendMarkRange("link").setLink({ href: url }).run();
      closeLinkDialog();
    }, [closeLinkDialog, editor, linkValue]);

    const openLinkDialog = useCallback(() => {
      setLinkError("");
      setLinkValue(editor?.getAttributes("link").href ?? "");
      setLinkDialogOpen(true);
    }, [editor]);

    const hideSelectionToolbar = useCallback(() => {
      selectionMenuArmedRef.current = false;
      editor?.commands.setMeta("clippingSelectionToolbar", "hide");
    }, [editor]);

    const showSelectionToolbarAfterInput = useCallback(() => {
      if (!editor || composingRef.current || readOnly) return;
      requestAnimationFrame(() => {
        if (editor.state.selection.empty) return;
        selectionMenuArmedRef.current = true;
        editor.commands.setMeta("clippingSelectionToolbar", "show");
        requestAnimationFrame(() => {
          if (!editor.isDestroyed && !editor.state.selection.empty) {
            editor.commands.setMeta("clippingSelectionToolbar", "updatePosition");
          }
        });
      });
    }, [editor, readOnly]);

    const handlePasteCapture = useCallback((event: React.ClipboardEvent<HTMLDivElement>) => {
      const transfer = event.clipboardData;
      const containsFile = Array.from(transfer.items).some((item) => item.kind === "file") || transfer.files.length > 0;
      if (containsFile) {
        event.preventDefault();
        setPasteNotice("Images aren't supported inside clipping notes.");
        return;
      }
      if (transfer.types.includes("text/html")) {
        event.preventDefault();
        const plainText = transfer.getData("text/plain");
        if (plainText && editor) editor.view.dispatch(editor.state.tr.insertText(plainText));
      }
    }, [editor]);

    const handleLinkDialogKeyDown = useCallback((event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeLinkDialog();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = Array.from(event.currentTarget.querySelectorAll<HTMLElement>(
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

    const selectionToolbarOptions = useMemo(() => ({
      strategy: "fixed" as const,
      placement: "top-start" as const,
      offset: 8,
      flip: true,
      shift: { padding: 8 }
    }), []);
    const selectionToolbarReference = useCallback(
      () => editor ? selectionStartVirtualElement(editor) : null,
      [editor]
    );
    const selectionToolbarContainer = useCallback(() => document.body, []);
    const shouldShowSelectionToolbar = useCallback(({ from, to }: { from: number; to: number }) => (
      selectionMenuArmedRef.current
      && !composingRef.current
      && from !== to
      && Boolean(editor?.isEditable)
    ), [editor]);
    const controlsDisabled = !editor || readOnly;

    return (
      <div
        aria-label="Clipping note editor"
        className="clipping-note-editor"
        data-editor-root="true"
        data-editor-transaction={transactionRevision}
        onCompositionEndCapture={() => flushCompletedComposition(documentId)}
        onCompositionStartCapture={() => {
          composingRef.current = true;
          hideSelectionToolbar();
        }}
        onKeyUpCapture={(event) => {
          const extendedSelection = event.shiftKey
            && ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key);
          const selectedAll = (event.ctrlKey || event.metaKey) && event.key.toLocaleLowerCase() === "a";
          if (extendedSelection || selectedAll) {
            showSelectionToolbarAfterInput();
          }
        }}
        onPasteCapture={handlePasteCapture}
        onPointerDownCapture={(event) => {
          if ((event.target as Element).closest("[data-selection-toolbar='true']")) return;
          hideSelectionToolbar();
        }}
        onPointerUpCapture={(event) => {
          if (event.button === 0) showSelectionToolbarAfterInput();
        }}
      >
        {editor && !readOnly ? (
          <BubbleMenu
            appendTo={selectionToolbarContainer}
            editor={editor}
            getReferencedVirtualElement={selectionToolbarReference}
            options={selectionToolbarOptions}
            pluginKey="clippingSelectionToolbar"
            shouldShow={shouldShowSelectionToolbar}
            updateDelay={0}
          >
            <div
              aria-label="Selected text formatting"
              className="clipping-note-editor__selection-toolbar"
              data-selection-toolbar="true"
              role="toolbar"
            >
              <IconToolbarButton active={editor.isActive("bold")} icon={Bold} label="Bold" onClick={() => editor.chain().focus().toggleBold().run()} />
              <IconToolbarButton active={editor.isActive("italic")} icon={Italic} label="Italic" onClick={() => editor.chain().focus().toggleItalic().run()} />
              <IconToolbarButton active={editor.isActive("strike")} icon={Strikethrough} label="Strikethrough" onClick={() => editor.chain().focus().toggleStrike().run()} />
              <span aria-hidden="true" className="clipping-note-editor__toolbar-rule" />
              <IconToolbarButton active={editor.isActive("link")} icon={Link2} label="Link" onClick={openLinkDialog} />
            </div>
          </BubbleMenu>
        ) : null}

        {linkDialogOpen ? (
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
            {linkError ? <p id="clipping-note-editor-link-error" role="alert">{linkError}</p> : null}
            <div className="clipping-note-editor__link-actions">
              <button onClick={submitLink} type="button">Apply link</button>
              <button onClick={closeLinkDialog} type="button">Cancel</button>
            </div>
          </div>
        ) : null}

        {pasteNotice ? <p className="clipping-note-editor__notice" role="status">{pasteNotice}</p> : null}
        <EditorContent editor={editor} />
        <footer className="clipping-note-editor__footer">
          <span>{!readOnly ? <>Type <kbd>/</kbd> for commands</> : "Read only"}</span>
          <div className="clipping-note-editor__footer-actions">
            {footerContent}
            <div aria-label="Editing history" className="clipping-note-editor__history" role="toolbar">
              <IconToolbarButton
                disabled={controlsDisabled || !editor?.can().undo()}
                icon={Undo2}
                label="Undo"
                onClick={() => editor?.chain().focus().undo().run()}
              />
              <IconToolbarButton
                disabled={controlsDisabled || !editor?.can().redo()}
                icon={Redo2}
                label="Redo"
                onClick={() => editor?.chain().focus().redo().run()}
              />
            </div>
          </div>
        </footer>
      </div>
    );
  }
);

type IconToolbarButtonProps = {
  active?: boolean;
  disabled?: boolean;
  icon: LucideIcon;
  label: string;
  onClick: () => void;
};

function IconToolbarButton({ active = false, disabled = false, icon: Icon, label, onClick }: IconToolbarButtonProps) {
  return (
    <button
      aria-label={label}
      aria-pressed={active}
      disabled={disabled}
      onClick={onClick}
      onPointerDown={(event) => {
        event.preventDefault();
        event.stopPropagation();
      }}
      title={label}
      type="button"
    >
      <Icon aria-hidden="true" />
    </button>
  );
}
