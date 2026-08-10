import { Extension, type Editor, type Range } from "@tiptap/core";
import { ReactRenderer } from "@tiptap/react";
import Suggestion, {
  type SuggestionKeyDownProps,
  type SuggestionProps
} from "@tiptap/suggestion";
import {
  Heading1,
  Heading2,
  Heading3,
  Heading4,
  List,
  ListOrdered,
  ListTodo,
  Pilcrow,
  Quote,
  SeparatorHorizontal,
  type LucideIcon
} from "lucide-react";
import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useState
} from "react";

type SlashCommandItem = {
  title: string;
  description: string;
  aliases: string[];
  icon: LucideIcon;
  command: (props: { editor: Editor; range: Range }) => void;
};

type SlashCommandMenuHandle = {
  onKeyDown: (event: KeyboardEvent) => boolean;
};

const SLASH_COMMAND_ITEMS: SlashCommandItem[] = [
  {
    title: "Text",
    description: "Continue with a plain paragraph.",
    aliases: ["paragraph", "body", "p", "plain", "normal"],
    icon: Pilcrow,
    command: ({ editor, range }) => editor.chain().focus().deleteRange(range).setParagraph().run()
  },
  {
    title: "To-do list",
    description: "Track an item that can be checked off.",
    aliases: ["todo", "task", "tasks", "check", "checklist", "checkbox"],
    icon: ListTodo,
    command: ({ editor, range }) => editor.chain().focus().deleteRange(range).toggleTaskList().run()
  },
  ...([1, 2, 3, 4] as const).map((level) => ({
    title: `Heading ${level}`,
    description: level === 1 ? "Start a major section." : `Create a level ${level} section.`,
    aliases: level === 1
      ? ["h", "h1", "heading", "header", "title", "big", "large"]
      : [`h${level}`, `heading${level}`, `header${level}`, "section", level === 2 ? "subtitle" : "small"],
    icon: [Heading1, Heading2, Heading3, Heading4][level - 1],
    command: ({ editor, range }: { editor: Editor; range: Range }) => (
      editor.chain().focus().deleteRange(range).setHeading({ level }).run()
    )
  })),
  {
    title: "Bullet list",
    description: "Create a simple list.",
    aliases: ["unordered", "bullets", "points", "ul"],
    icon: List,
    command: ({ editor, range }) => editor.chain().focus().deleteRange(range).toggleBulletList().run()
  },
  {
    title: "Numbered list",
    description: "Create a list with numbering.",
    aliases: ["ordered", "numbers", "ol"],
    icon: ListOrdered,
    command: ({ editor, range }) => editor.chain().focus().deleteRange(range).toggleOrderedList().run()
  },
  {
    title: "Quote",
    description: "Set a passage apart from your notes.",
    aliases: ["blockquote", "citation"],
    icon: Quote,
    command: ({ editor, range }) => editor.chain().focus().deleteRange(range).toggleBlockquote().run()
  },
  {
    title: "Divider",
    description: "Separate one part of the note from the next.",
    aliases: ["rule", "separator", "line", "hr", "break"],
    icon: SeparatorHorizontal,
    command: ({ editor, range }) => editor.chain().focus().deleteRange(range).setHorizontalRule().run()
  }
];

function normalizeSlashSearch(value: string) {
  return value.normalize("NFKC").toLocaleLowerCase().replace(/[^\p{L}\p{N}]+/gu, "");
}

function editDistance(left: string, right: string) {
  const previous = Array.from({ length: right.length + 1 }, (_, index) => index);
  for (let leftIndex = 1; leftIndex <= left.length; leftIndex += 1) {
    const current = [leftIndex];
    for (let rightIndex = 1; rightIndex <= right.length; rightIndex += 1) {
      current[rightIndex] = Math.min(
        current[rightIndex - 1] + 1,
        previous[rightIndex] + 1,
        previous[rightIndex - 1] + (left[leftIndex - 1] === right[rightIndex - 1] ? 0 : 1)
      );
    }
    previous.splice(0, previous.length, ...current);
  }
  return previous[right.length];
}

function subsequenceGap(candidate: string, query: string) {
  let candidateIndex = 0;
  let firstMatch = -1;
  let lastMatch = -1;
  for (const character of query) {
    const match = candidate.indexOf(character, candidateIndex);
    if (match === -1) return null;
    if (firstMatch === -1) firstMatch = match;
    lastMatch = match;
    candidateIndex = match + 1;
  }
  return firstMatch + Math.max(0, lastMatch - firstMatch + 1 - query.length);
}

function slashCandidateScore(candidate: string, query: string) {
  if (candidate === query) return 0;
  if (candidate.startsWith(query)) return 10 + candidate.length - query.length;
  const containedAt = candidate.indexOf(query);
  if (containedAt >= 0) return 30 + containedAt + candidate.length - query.length;
  const gap = subsequenceGap(candidate, query);
  if (gap !== null) return 50 + gap + candidate.length - query.length;
  const distance = editDistance(candidate, query);
  const allowedDistance = Math.max(1, Math.floor(query.length / 3));
  return distance <= allowedDistance
    ? 70 + distance * 5 + Math.abs(candidate.length - query.length)
    : Number.POSITIVE_INFINITY;
}

function rankSlashCommandItems(query: string) {
  const normalizedQuery = normalizeSlashSearch(query);
  if (!normalizedQuery) return SLASH_COMMAND_ITEMS;
  return SLASH_COMMAND_ITEMS
    .map((item, index) => ({
      index,
      item,
      score: Math.min(...[item.title, ...item.aliases].map((candidate) => (
        slashCandidateScore(normalizeSlashSearch(candidate), normalizedQuery)
      )))
    }))
    .filter(({ score }) => Number.isFinite(score))
    .sort((left, right) => left.score - right.score || left.index - right.index)
    .map(({ item }) => item);
}

const SlashCommandMenu = forwardRef<SlashCommandMenuHandle, SuggestionProps<SlashCommandItem, SlashCommandItem>>(
  function SlashCommandMenu({ command, items, query }, forwardedRef) {
    const [selectedIndex, setSelectedIndex] = useState(0);
    const activeIndex = items.length === 0 ? -1 : Math.min(selectedIndex, items.length - 1);

    useEffect(() => setSelectedIndex(0), [query]);

    const selectItem = useCallback((index: number) => {
      const item = items[index];
      if (item) command(item);
    }, [command, items]);

    useImperativeHandle(forwardedRef, () => ({
      onKeyDown: (event) => {
        if (event.key === "ArrowUp") {
          setSelectedIndex((current) => items.length === 0 ? 0 : (current - 1 + items.length) % items.length);
          return true;
        }
        if (event.key === "ArrowDown") {
          setSelectedIndex((current) => items.length === 0 ? 0 : (current + 1) % items.length);
          return true;
        }
        if (event.key === "Enter") {
          if (activeIndex >= 0) selectItem(activeIndex);
          return true;
        }
        return false;
      }
    }), [activeIndex, items.length, selectItem]);

    return (
      <div aria-label="Insert a note block" className="clipping-note-editor__slash-menu" role="listbox">
        <div className="clipping-note-editor__slash-heading">
          <span>Turn into</span>
          <kbd>Esc</kbd>
        </div>
        {items.length === 0 ? (
          <p className="clipping-note-editor__slash-empty">No matching commands</p>
        ) : items.map((item, index) => {
          const Icon = item.icon;
          const selected = index === activeIndex;
          return (
            <button
              aria-selected={selected}
              className="clipping-note-editor__slash-item"
              key={item.title}
              onClick={() => selectItem(index)}
              onMouseEnter={() => setSelectedIndex(index)}
              onPointerDown={(event) => event.preventDefault()}
              role="option"
              type="button"
            >
              <span className="clipping-note-editor__slash-icon"><Icon aria-hidden="true" /></span>
              <span>
                <strong>{item.title}</strong>
                <small>{item.description}</small>
              </span>
            </button>
          );
        })}
      </div>
    );
  }
);

export function createClippingNoteSlashCommandExtension() {
  return Extension.create({
    name: "clippingSlashCommand",
    addProseMirrorPlugins() {
      return [Suggestion<SlashCommandItem, SlashCommandItem>({
        editor: this.editor,
        char: "/",
        startOfLine: false,
        allowedPrefixes: null,
        decorationClass: "clipping-note-editor__slash-query",
        placement: "bottom-start",
        offset: { mainAxis: 8, crossAxis: 0 },
        flip: true,
        container: "body",
        floatingUi: { strategy: "fixed" },
        allow: ({ editor, state, range }) => {
          if (!editor.isEditable) return false;
          const position = state.doc.resolve(range.from);
          const parent = position.parent;
          const textBeforeSlash = parent.textBetween(0, position.parentOffset, "\0", "\0");
          return parent.isTextblock && (textBeforeSlash.length === 0 || /\s$/.test(textBeforeSlash));
        },
        items: ({ query }) => rankSlashCommandItems(query),
        command: ({ editor, range, props }) => props.command({ editor, range }),
        render: () => {
          let component: ReactRenderer<
            SlashCommandMenuHandle,
            SuggestionProps<SlashCommandItem, SlashCommandItem>
          > | null = null;
          let unmount: (() => void) | null = null;
          return {
            onStart: (props) => {
              component = new ReactRenderer<
                SlashCommandMenuHandle,
                SuggestionProps<SlashCommandItem, SlashCommandItem>
              >(SlashCommandMenu, { editor: props.editor, props });
              component.element.classList.add("clipping-note-editor__slash-popover");
              unmount = props.mount(component.element);
            },
            onUpdate: (props) => component?.updateProps(props),
            onKeyDown: ({ event }: SuggestionKeyDownProps) => component?.ref?.onKeyDown(event) ?? false,
            onExit: () => {
              unmount?.();
              component?.destroy();
              unmount = null;
              component = null;
            }
          };
        }
      })];
    }
  });
}
