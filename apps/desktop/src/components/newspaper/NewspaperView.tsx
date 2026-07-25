import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  CalendarClock,
  ChevronLeft,
  ChevronRight,
  Download,
  FolderOpen,
  LoaderCircle,
  Maximize2,
  Pause,
  Play,
  RotateCcw,
  Search,
  X,
  ZoomIn,
  ZoomOut
} from "lucide-react";
import { toast } from "sonner";
import { Button, Checkbox, Input, Select, StatusBadge, Switch } from "../primitives";

type EditionKind = "daily" | "weekly" | "special";
type NewspaperEdition = {
  code: string;
  nameZh: string;
  nameEn: string;
  kind: EditionKind;
  schedule: "daily" | "weekly_sunday" | "ad_hoc";
  sourceUrl: string;
  publicationDate?: string | null;
  discovered: boolean;
};
type NewspaperBatch = {
  id: string;
  status: string;
  destination: string;
  scheduled_at?: number | null;
  delay_minutes: number;
};
type NewspaperJob = {
  id: string;
  batch_id: string;
  edition_code: string;
  edition_name: string;
  publication_date: string;
  status: string;
  output_dir: string;
  page_count: number;
  completed_count: number;
  failed_count: number;
  warning?: string | null;
  updated_at: number;
};
type NewspaperPage = {
  id: string;
  page_number: string;
  section_name?: string | null;
  status: string;
  display_path?: string | null;
};
type Bootstrap = {
  catalog: NewspaperEdition[];
  batches: NewspaperBatch[];
  jobs: NewspaperJob[];
  settings: Record<string, unknown>;
};

const PREF_KEY = "linkvault.newspaper.preferences";
const FALLBACK_CATALOG: NewspaperEdition[] = [
  ["NY", "紐約", "New York", "daily"],
  ["LA", "洛杉磯", "Los Angeles", "daily"],
  ["SF", "舊金山", "San Francisco", "daily"],
  ["NJ", "新賓", "New Jersey / Pennsylvania", "daily"],
  ["DC", "大華府", "Washington, D.C.", "daily"],
  ["BO", "波士頓", "Boston", "daily"],
  ["AT", "美東南", "Southeast U.S.", "daily"],
  ["CH", "芝加哥", "Chicago", "daily"],
  ["TX", "德州", "Texas", "daily"],
  ["SE", "西雅圖／夏威夷", "Seattle / Hawaii", "daily"],
  ["NW", "世界周刊（美東）", "Weekly — East", "weekly"],
  ["LW", "世界周刊（美西南）", "Weekly — Southwest", "weekly"],
  ["SW", "世界周刊（美西北）", "Weekly — Northwest", "weekly"]
].map(([code, nameZh, nameEn, kind]) => ({
  code,
  nameZh,
  nameEn,
  kind: kind as EditionKind,
  schedule: kind === "weekly" ? "weekly_sunday" : "daily",
  sourceUrl: `https://ep.worldjournal.com/${code}`,
  discovered: false
}));

function today() {
  return new Date().toISOString().slice(0, 10);
}

function isTauriRuntime() {
  return "__TAURI_INTERNALS__" in window;
}

function editionKey(edition: NewspaperEdition) {
  return edition.publicationDate ? `${edition.code}@${edition.publicationDate}` : edition.code;
}

export function NewspaperView({ mode = "download" }: { mode?: "download" | "library" }) {
  const initial = useRef(readPreferences());
  const [catalog, setCatalog] = useState<NewspaperEdition[]>(FALLBACK_CATALOG);
  const [batches, setBatches] = useState<NewspaperBatch[]>([]);
  const [jobs, setJobs] = useState<NewspaperJob[]>([]);
  const [selected, setSelected] = useState<Set<string>>(() => new Set(initial.current.selected ?? ["NY"]));
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState<"all" | EditionKind>("all");
  const [libraryStatus, setLibraryStatus] = useState<"all" | "completed" | "partial">("all");
  const [libraryLimit, setLibraryLimit] = useState(50);
  const [dateMode, setDateMode] = useState<"single" | "last_7_days" | "custom">("single");
  const [startDate, setStartDate] = useState(today());
  const [endDate, setEndDate] = useState(today());
  const [destination, setDestination] = useState(initial.current.destination ?? "");
  const [delayMinutes, setDelayMinutes] = useState(initial.current.delayMinutes ?? 5);
  const [optimize, setOptimize] = useState(initial.current.optimize ?? true);
  const [profile, setProfile] = useState(initial.current.profile ?? "webp_high");
  const [keepOriginal, setKeepOriginal] = useState(initial.current.keepOriginal ?? false);
  const [schedule, setSchedule] = useState(initial.current.schedule ?? false);
  const [scheduledLocal, setScheduledLocal] = useState(initial.current.scheduledLocal ?? "");
  const [submitting, setSubmitting] = useState(false);
  const [processing, setProcessing] = useState(false);
  const [previews, setPreviews] = useState<Record<string, string>>({});
  const [reader, setReader] = useState<{ job: NewspaperJob; pages: NewspaperPage[]; index: number; image: string; zoom: number } | null>(null);

  async function refresh() {
    if (!isTauriRuntime()) return;
    try {
      const state = await invoke<Bootstrap>("bootstrap_newspaper_state");
      setCatalog(state.catalog.length ? state.catalog : FALLBACK_CATALOG);
      setBatches(state.batches);
      setJobs(state.jobs);
    } catch (error) {
      toast.error("Could not load newspaper state", { description: String(error) });
    }
  }

  useEffect(() => {
    if (isTauriRuntime()) {
      void invoke<NewspaperEdition[]>("refresh_newspaper_catalog")
        .then((items) => items.length && setCatalog(items))
        .catch(() => undefined)
        .finally(() => void refresh());
    } else {
      void refresh();
    }
    if (!isTauriRuntime()) return;
    const timer = window.setInterval(() => {
      void refresh();
      void invoke("process_newspaper_queue").catch(() => undefined);
    }, 5_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    window.localStorage.setItem(PREF_KEY, JSON.stringify({
      destination,
      delayMinutes,
      optimize,
      profile,
      keepOriginal,
      schedule,
      scheduledLocal,
      selected: [...selected]
    }));
  }, [delayMinutes, destination, keepOriginal, optimize, profile, schedule, scheduledLocal, selected]);

  const visibleEditions = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return catalog.filter((edition) =>
      (kind === "all" || edition.kind === kind)
      && (!needle || `${edition.code} ${edition.nameZh} ${edition.nameEn}`.toLowerCase().includes(needle))
    );
  }, [catalog, kind, query]);
  const activeJobs = jobs.filter((job) => !["completed", "partial", "failed", "unavailable", "cancelled"].includes(job.status));
  const editionKinds = new Map(catalog.map((edition) => [edition.code, edition.kind]));
  const libraryJobs = jobs
    .filter((job) => ["completed", "partial"].includes(job.status))
    .filter((job) => libraryStatus === "all" || job.status === libraryStatus)
    .filter((job) => kind === "all" || editionKinds.get(job.edition_code) === kind)
    .filter((job) => !query || `${job.edition_name} ${job.edition_code} ${job.publication_date}`.toLowerCase().includes(query.toLowerCase()))
    .sort((a, b) => b.publication_date.localeCompare(a.publication_date));

  useEffect(() => {
    if (mode !== "library" || !isTauriRuntime()) return;
    for (const job of libraryJobs.slice(0, 50)) {
      if (previews[job.id]) continue;
      void invoke<string>("get_newspaper_preview", { jobId: job.id })
        .then((value) => setPreviews((current) => ({ ...current, [job.id]: value })))
        .catch(() => undefined);
    }
  }, [libraryJobs, mode, previews]);

  async function chooseFolder() {
    const picked = await open({ directory: true, multiple: false, title: "Choose newspaper folder" });
    if (typeof picked === "string") setDestination(picked);
  }

  async function registerArchive() {
    const picked = await open({ directory: true, multiple: false, title: "Register existing newspaper archive" });
    if (typeof picked !== "string" || !isTauriRuntime()) return;
    try {
      const count = await invoke<number>("import_existing_newspaper_archive", { path: picked });
      toast.success(`Registered ${count} newspaper editions`);
      await refresh();
    } catch (error) {
      toast.error("Could not register newspaper archive", { description: String(error) });
    }
  }

  async function submit() {
    if (!destination.trim()) {
      toast.warning("Choose a download folder");
      return;
    }
    if (selected.size === 0) {
      toast.warning("Select at least one edition");
      return;
    }
    if (!isTauriRuntime()) {
      toast.info("Browser preview", { description: "Run the Tauri app to download newspapers." });
      return;
    }
    setSubmitting(true);
    try {
      const scheduledAt = schedule && scheduledLocal
        ? Math.floor(new Date(scheduledLocal).getTime() / 1000)
        : undefined;
      await invoke("create_newspaper_batch", {
        request: {
          editionCodes: [...selected],
          dateMode,
          startDate,
          endDate: dateMode === "custom" ? endDate : undefined,
          destination,
          scheduledAt,
          delayMinutes,
          optimizeImages: optimize,
          optimizationProfile: profile,
          keepOriginalJpg: keepOriginal
        }
      });
      toast.success(schedule ? "Downloads scheduled" : "Newspaper download queued");
      await refresh();
      if (!schedule) {
        setProcessing(true);
        await invoke("process_newspaper_queue");
        await refresh();
      }
    } catch (error) {
      toast.error("Could not start newspaper download", { description: String(error) });
    } finally {
      setSubmitting(false);
      setProcessing(false);
    }
  }

  async function openReader(job: NewspaperJob) {
    if (!isTauriRuntime()) return;
    try {
      const pages = await invoke<NewspaperPage[]>("get_newspaper_reader_manifest", { jobId: job.id });
      const first = pages.findIndex((page) => page.status === "completed");
      if (first < 0) throw new Error("No completed pages are available.");
      const image = await invoke<string>("get_newspaper_page_image", { pageId: pages[first].id });
      setReader({ job, pages, index: first, image, zoom: 1 });
    } catch (error) {
      toast.error("Could not open newspaper", { description: String(error) });
    }
  }

  async function changeReaderPage(nextIndex: number) {
    if (!reader) return;
    const bounded = Math.max(0, Math.min(reader.pages.length - 1, nextIndex));
    const page = reader.pages[bounded];
    if (page.status !== "completed") return;
    const image = await invoke<string>("get_newspaper_page_image", { pageId: page.id });
    setReader({ ...reader, index: bounded, image });
  }

  useEffect(() => {
    if (!reader) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        void changeReaderPage(reader.index - 1);
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        void changeReaderPage(reader.index + 1);
      } else if (event.key === "Escape") {
        setReader(null);
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [reader]);

  if (mode === "library") {
    return (
      <section className="newspaper-library" aria-label="Newspaper library">
        <div className="newspaper-library-toolbar">
          <label className="newspaper-search">
            <Search aria-hidden="true" />
            <Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search editions or dates" aria-label="Search newspaper library" />
          </label>
          <Select value={kind} onChange={(event) => setKind(event.target.value as typeof kind)} aria-label="Filter newspaper kind">
            <option value="all">All publications</option>
            <option value="daily">Daily</option>
            <option value="weekly">Weekly</option>
            <option value="special">Special</option>
          </Select>
          <Select value={libraryStatus} onChange={(event) => setLibraryStatus(event.target.value as typeof libraryStatus)} aria-label="Filter newspaper status">
            <option value="all">All statuses</option>
            <option value="completed">Completed</option>
            <option value="partial">Partial</option>
          </Select>
          <Button variant="outline" onClick={() => void registerArchive()}><FolderOpen /> Register archive</Button>
        </div>
        <div className="newspaper-library-list">
          {libraryJobs.length === 0 ? <div className="newspaper-empty">Downloaded editions will appear here.</div> : null}
          {libraryJobs.slice(0, libraryLimit).map((job) => (
            <article className="newspaper-library-row" key={job.id}>
              <div className="newspaper-preview">
                {previews[job.id] ? <img src={previews[job.id]} alt={`${job.edition_name} front page preview`} /> : <span>{job.edition_code}</span>}
              </div>
              <div className="newspaper-library-copy">
                <strong>{job.edition_name}</strong>
                <span>{job.edition_code} · {job.publication_date} · {job.completed_count}/{job.page_count} pages</span>
                {job.warning ? <small>{job.warning}</small> : null}
              </div>
              <StatusBadge tone={job.status === "completed" ? "success" : "danger"}>{job.status}</StatusBadge>
              <div className="newspaper-row-actions">
                {job.status === "partial" ? (
                  <Button size="xs" variant="ghost" onClick={() => void invoke("retry_newspaper_job", { jobId: job.id }).then(refresh)}>
                    <RotateCcw /> Retry missing
                  </Button>
                ) : null}
                <Button size="xs" variant="ghost" onClick={() => void invoke("open_newspaper_download_folder", { path: job.output_dir })}><FolderOpen /> Folder</Button>
                <Button size="xs" onClick={() => void openReader(job)}>Read</Button>
              </div>
            </article>
          ))}
          {libraryJobs.length > libraryLimit ? (
            <Button className="newspaper-library-more" variant="outline" onClick={() => setLibraryLimit((current) => current + 50)}>
              Load 50 more
            </Button>
          ) : null}
        </div>
        {reader ? (
          <div className="newspaper-reader-backdrop" role="presentation">
            <section className="newspaper-reader" role="dialog" aria-modal="true" aria-label={`${reader.job.edition_name} reader`}>
              <header>
                <div><strong>{reader.job.edition_name}</strong><span>{reader.job.publication_date} · {reader.pages[reader.index]?.page_number}</span></div>
                <div className="newspaper-reader-controls">
                  <Button size="icon-sm" variant="ghost" aria-label="Previous page" onClick={() => void changeReaderPage(reader.index - 1)}><ChevronLeft /></Button>
                  <Select
                    value={String(reader.index)}
                    onChange={(event) => void changeReaderPage(Number(event.target.value))}
                    aria-label="Select newspaper page"
                  >
                    {reader.pages.map((page, index) => (
                      <option key={page.id} value={index} disabled={page.status !== "completed"}>
                        {page.page_number}
                      </option>
                    ))}
                  </Select>
                  <span>/ {reader.pages.length}</span>
                  <Button size="icon-sm" variant="ghost" aria-label="Next page" onClick={() => void changeReaderPage(reader.index + 1)}><ChevronRight /></Button>
                  <Button size="icon-sm" variant="ghost" aria-label="Zoom out" onClick={() => setReader({ ...reader, zoom: Math.max(.5, reader.zoom - .25) })}><ZoomOut /></Button>
                  <Button size="icon-sm" variant="ghost" aria-label="Zoom in" onClick={() => setReader({ ...reader, zoom: Math.min(3, reader.zoom + .25) })}><ZoomIn /></Button>
                  <Button size="icon-sm" variant="ghost" aria-label="Fit page width" onClick={() => setReader({ ...reader, zoom: 1 })}><Maximize2 /></Button>
                  <Button size="icon-sm" variant="ghost" aria-label="Close reader" onClick={() => setReader(null)}><X /></Button>
                </div>
              </header>
              <div className="newspaper-reader-canvas"><img src={reader.image} alt={`Page ${reader.pages[reader.index]?.page_number}`} style={{ width: `${reader.zoom * 100}%` }} /></div>
            </section>
          </div>
        ) : null}
      </section>
    );
  }

  return (
    <section className="newspaper-download" aria-label="Download World Journal editions">
      <div className="newspaper-target-row">
        <label><span>Save to</span><Input value={destination} onChange={(event) => setDestination(event.target.value)} placeholder="Choose a newspaper folder" /></label>
        <Button variant="outline" onClick={() => void chooseFolder()}><FolderOpen /> Browse</Button>
      </div>
      <div className="newspaper-setup-grid">
        <div className="newspaper-editions">
          <div className="newspaper-edition-tools">
            <label className="newspaper-search"><Search aria-hidden="true" /><Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search editions" aria-label="Search editions" /></label>
            <Select value={kind} onChange={(event) => setKind(event.target.value as typeof kind)} aria-label="Filter edition kind">
              <option value="all">All</option><option value="daily">Daily</option><option value="weekly">Weekly</option><option value="special">Special</option>
            </Select>
            <button type="button" onClick={() => setSelected(new Set(catalog.filter((item) => item.kind === "daily").map(editionKey)))}>All daily</button>
          </div>
          <div className="newspaper-edition-list">
            {visibleEditions.map((edition) => {
              const key = editionKey(edition);
              return (
                <Checkbox key={key} label={`${edition.nameZh} · ${edition.code}`} checked={selected.has(key)} onChange={(event) => {
                  const next = new Set(selected);
                  if (event.target.checked) next.add(key); else next.delete(key);
                  setSelected(next);
                }} />
              );
            })}
          </div>
        </div>
        <div className="newspaper-options">
          <div className="newspaper-option-line">
            <span>Date</span>
            <Select value={dateMode} onChange={(event) => setDateMode(event.target.value as typeof dateMode)}>
              <option value="single">Single date</option><option value="last_7_days">Last 7 days</option><option value="custom">Custom range</option>
            </Select>
          </div>
          <div className="newspaper-date-line">
            <Input type="date" value={startDate} onChange={(event) => setStartDate(event.target.value)} aria-label="Start publication date" />
            {dateMode === "custom" ? <Input type="date" value={endDate} onChange={(event) => setEndDate(event.target.value)} aria-label="End publication date" /> : null}
          </div>
          <div className="newspaper-option-line">
            <span>Delay between editions</span>
            <Input className="newspaper-delay-input" type="number" min={0} max={1440} value={delayMinutes} onChange={(event) => setDelayMinutes(Number(event.target.value))} />
            <span>minutes</span>
          </div>
          <div className="newspaper-option-line">
            <Switch label="Optimize images" checked={optimize} onChange={(event) => setOptimize(event.target.checked)} />
            <Select value={profile} onChange={(event) => setProfile(event.target.value)} disabled={!optimize} aria-label="Image optimization profile">
              <option value="webp_high">High clarity · WebP 92</option>
              <option value="webp_balanced">Balanced · WebP 86</option>
            </Select>
          </div>
          <Checkbox label="Keep original JPG files" checked={keepOriginal} onChange={(event) => setKeepOriginal(event.target.checked)} disabled={!optimize} />
        </div>
      </div>
      <div className="newspaper-action-row">
        <Checkbox label="Schedule download" checked={schedule} onChange={(event) => setSchedule(event.target.checked)} />
        {schedule ? <Input type="datetime-local" value={scheduledLocal} onChange={(event) => setScheduledLocal(event.target.value)} aria-label="Scheduled local date and time" /> : null}
        <Button variant="primary" loading={submitting || processing} onClick={() => void submit()}>
          {schedule ? <CalendarClock /> : <Download />}
          {schedule ? "Schedule downloads" : "Download now"}
        </Button>
      </div>
      <div className="newspaper-active-table">
        <div className="newspaper-active-head"><span>Status</span><span>Edition</span><span>Date</span><span>Pages</span><span>Actions</span></div>
        {activeJobs.length === 0 ? <div className="newspaper-empty">No active newspaper downloads.</div> : activeJobs.map((job) => {
          const batch = batches.find((item) => item.id === job.batch_id);
          return (
            <div className="newspaper-active-row" key={job.id}>
              <StatusBadge tone="primary">{job.status}</StatusBadge>
              <strong>{job.edition_name}</strong><span>{job.publication_date}</span>
              <span>{job.completed_count}/{job.page_count || "—"}</span>
              <div>
                <Button size="icon-sm" variant="ghost" aria-label={batch?.status === "paused" ? "Resume batch" : "Pause batch"} onClick={() => void invoke("pause_newspaper_batch", { batchId: job.batch_id, paused: batch?.status !== "paused" }).then(refresh)}>
                  {batch?.status === "paused" ? <Play /> : <Pause />}
                </Button>
                <Button size="icon-sm" variant="ghost" aria-label="Cancel batch" onClick={() => void invoke("cancel_newspaper_batch", { batchId: job.batch_id }).then(refresh)}>
                  <X />
                </Button>
              </div>
            </div>
          );
        })}
      </div>
      {processing ? <div className="newspaper-processing"><LoaderCircle className="lv-button-spinner" /> Downloading and validating pages…</div> : null}
    </section>
  );
}

function readPreferences(): {
  destination?: string;
  delayMinutes?: number;
  optimize?: boolean;
  profile?: string;
  keepOriginal?: boolean;
  schedule?: boolean;
  scheduledLocal?: string;
  selected?: string[];
} {
  try {
    return JSON.parse(window.localStorage.getItem(PREF_KEY) ?? "{}");
  } catch {
    return {};
  }
}
