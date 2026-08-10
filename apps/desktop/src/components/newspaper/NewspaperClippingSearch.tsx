import { FileSearch, LoaderCircle, SearchX } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import {
  searchNewspaperClippings,
  searchPossibleNewspaperClippings,
  type NewspaperClippingSearchResult
} from "./newspaper-api";

export function NewspaperClippingSearch({
  query,
  onOpen
}: {
  query: string;
  onOpen: (clippingId: string) => void;
}) {
  const [debounced, setDebounced] = useState("");
  const [items, setItems] = useState<NewspaperClippingSearchResult[]>([]);
  const [possible, setPossible] = useState<NewspaperClippingSearchResult[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const generationRef = useRef(0);
  const loadingOffsetRef = useRef<string | null>(null);
  const moreRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const bounded = [...query.trim()].slice(0, 200).join("");
    const timer = window.setTimeout(() => setDebounced(bounded), 200);
    return () => window.clearTimeout(timer);
  }, [query]);

  const loadPage = async (offset: number, generation: number) => {
    const owner = `${generation}:${offset}`;
    if (!debounced || loadingOffsetRef.current === owner) return;
    loadingOffsetRef.current = owner;
    setLoading(true);
    try {
      const page = await searchNewspaperClippings(debounced, offset);
      if (generation !== generationRef.current) return;
      setTotal(page.total);
      setItems((current) => offset === 0 ? page.items : [...current, ...page.items]);
      setError("");
    } catch (cause) {
      if (generation === generationRef.current) setError(String(cause));
    } finally {
      if (loadingOffsetRef.current === owner) loadingOffsetRef.current = null;
      if (generation === generationRef.current) setLoading(false);
    }
  };

  useEffect(() => {
    const generation = generationRef.current + 1;
    generationRef.current = generation;
    setItems([]);
    setPossible([]);
    setTotal(0);
    setError("");
    if (!debounced) return;
    void loadPage(0, generation);
    void searchPossibleNewspaperClippings(debounced).then((response) => {
      if (generation === generationRef.current) setPossible(response.items.slice(0, 25));
    }).catch(() => {
      if (generation === generationRef.current) setPossible([]);
    });
  }, [debounced]);

  useEffect(() => {
    const target = moreRef.current;
    if (!target || items.length >= total || loading) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        void loadPage(items.length, generationRef.current);
      }
    }, { rootMargin: "240px" });
    observer.observe(target);
    return () => observer.disconnect();
  }, [items.length, loading, total]);

  if (!debounced) return null;

  return (
    <section className="clipping-search-results" aria-label="Clipping search results">
      <header>
        <div><span>Search across saved clipping metadata and your written notes</span><h2>Results for “{debounced}”</h2></div>
        <strong>{total} confident match{total === 1 ? "" : "es"}</strong>
      </header>
      {error ? <div className="clipping-search-state" role="alert">Search failed. {error}</div> : null}
      {!error && !loading && items.length === 0 && possible.length === 0 ? (
        <div className="clipping-search-state"><SearchX aria-hidden="true" /> No saved clipping matches this keyword.</div>
      ) : null}
      <div className="clipping-search-list">
        {items.map((result) => <SearchResult key={result.clipping.id} result={result} onOpen={onOpen} />)}
      </div>
      <div ref={moreRef} className="clipping-search-more">
        {loading ? <><LoaderCircle aria-hidden="true" className="animate-spin" /> Loading results…</> : null}
      </div>
      {possible.length ? (
        <section className="clipping-possible-results" aria-label="Possible matches">
          <header><FileSearch aria-hidden="true" /><div><strong>Possible matches</strong><span>Lower-confidence fuzzy title, note, or edition matches</span></div></header>
          <div className="clipping-search-list">
            {possible.map((result) => <SearchResult key={result.clipping.id} result={result} onOpen={onOpen} />)}
          </div>
        </section>
      ) : null}
    </section>
  );
}

function SearchResult({ result, onOpen }: { result: NewspaperClippingSearchResult; onOpen: (id: string) => void }) {
  return (
    <button className="clipping-search-row" onClick={() => onOpen(result.clipping.id)} type="button">
      <span className="clipping-search-row__meta">
        <strong>{result.clipping.title}</strong>
        <span>{result.clipping.editionName} · {result.clipping.publicationDate} · page {result.clipping.pageNumber}</span>
      </span>
      <span className="clipping-search-row__tags" aria-label="Matched fields">
        {result.matchedFields.map((field) => <span key={field}>{field === "note" ? "Note" : field[0].toUpperCase() + field.slice(1)}</span>)}
      </span>
      <span className="clipping-search-row__snippet">
        {(result.snippets[0]?.parts ?? []).map((part, index) => part.highlighted
          ? <mark key={index}>{part.text}</mark>
          : <span key={index}>{part.text}</span>)}
      </span>
    </button>
  );
}
