import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import {
  sameSnapshotValue,
  sameSnapshotRecord,
  sameSnapshotList
} from "../src/components/newspaper/newspaper-activity-equality.ts";

const job = () => ({
  id: "job-1",
  batch_id: "batch-1",
  edition_code: "NY",
  status: "downloading",
  output_dir: "C:/newspapers/NY",
  page_count: 40,
  completed_count: 12,
  failed_count: 0,
  retry_at: null,
  warning: null,
  queue_position: 0,
  paused: false,
  dismissed: false,
  updated_at: 1_700_000_000
});

const progress = () => ({
  jobId: "job-1",
  currentStage: "download",
  downloadTotal: 40,
  downloadCompleted: 12,
  activeWorkers: 3,
  pagesPerMinute: null,
  etaSeconds: 90,
  optimizedBytes: 0
});

const schedule = () => ({
  id: "schedule-1",
  enabled: true,
  cron_time: "07:00",
  edition_codes: ["NY", "LA"],
  date_mode: "single",
  delay_seconds: 15,
  last_error: null
});

const runtime = () => ({
  active: true,
  mode: "auto",
  requestedWorkers: 8,
  admittedWorkers: 6,
  activeWorkers: 4,
  cpuPercent: 37.5,
  availableMemoryBytes: null,
  memorySafe: true,
  limitedReason: null
});

// Identical freshly deserialized records must compare equal so the poll can bail out.
assert.equal(sameSnapshotRecord(job(), job()), true, "Two identical job records must be equal");
assert.equal(sameSnapshotRecord(progress(), progress()), true, "Two identical progress records must be equal");
assert.equal(sameSnapshotRecord(runtime(), runtime()), true, "Two identical optimization runtimes must be equal");
assert.equal(
  sameSnapshotRecord({ ...job(), updated_at: 1_700_000_001 }, job()),
  false,
  "A single changed scalar field must not be equal"
);
assert.equal(
  sameSnapshotRecord({ ...runtime(), activeWorkers: 5 }, runtime()),
  false,
  "A changed optimization worker count must not be equal"
);
assert.equal(
  sameSnapshotRecord({ ...progress(), downloadCompleted: 13 }, progress()),
  false,
  "A changed progress counter must not be equal"
);

// A changed field count means a key was added or removed and must never compare equal.
const { warning, ...jobWithoutWarning } = job();
assert.equal(sameSnapshotRecord(jobWithoutWarning, job()), false, "A removed key must not be equal");
assert.equal(sameSnapshotRecord(job(), { ...jobWithoutWarning, warning: "disk full" }), false, "A re-added key with a new value must not be equal");
assert.equal(sameSnapshotRecord({ ...job(), edition_codes: ["NY"] }, job()), false, "An added key must not be equal even when its value is null");

// The one nested value is NewspaperSchedule.edition_codes, compared element by element.
assert.equal(sameSnapshotRecord(schedule(), schedule()), true, "Equal edition code lists must be equal");
assert.equal(
  sameSnapshotRecord({ ...schedule(), edition_codes: ["LA", "NY"] }, schedule()),
  false,
  "Reordered edition codes must not be equal"
);
assert.equal(
  sameSnapshotRecord({ ...schedule(), edition_codes: ["NY", "SF"] }, schedule()),
  false,
  "A changed edition code must not be equal"
);
assert.equal(
  sameSnapshotRecord({ ...schedule(), edition_codes: ["NY"] }, schedule()),
  false,
  "A shorter edition code list must not be equal"
);

// Optional fields arrive as either a missing key or an explicit null, and both mean "unset".
assert.equal(sameSnapshotValue(undefined, null), true, "A missing optional field must equal an explicit null");
assert.equal(sameSnapshotValue(null, undefined), true, "An explicit null must equal a missing optional field");
assert.equal(sameSnapshotRecord({ ...job(), retry_at: undefined }, job()), true, "An omitted nullable field must not force an update");
assert.equal(sameSnapshotRecord({ ...runtime(), cpuPercent: undefined }, { ...runtime(), cpuPercent: null }), true, "An omitted runtime metric must equal a null runtime metric");
assert.equal(sameSnapshotRecord({ ...schedule(), last_error: undefined }, { ...schedule(), last_error: null }), true, "An omitted schedule error must not force an update");

// Zero, empty string, and false are real values and must never be folded into "unset".
assert.equal(sameSnapshotValue(0, undefined), false, "Zero must not compare equal to unset");
assert.equal(sameSnapshotValue("", undefined), false, "An empty string must not compare equal to unset");
assert.equal(sameSnapshotValue(false, null), false, "False must not compare equal to unset");
assert.equal(sameSnapshotValue(0, null), false, "Zero must not compare equal to null");
assert.equal(sameSnapshotRecord({ ...runtime(), cpuPercent: 0 }, { ...runtime(), cpuPercent: null }), false, "A zero metric must not be folded into an unset metric");
assert.equal(sameSnapshotRecord({ ...runtime(), limitedReason: "" }, { ...runtime(), limitedReason: null }), false, "An empty reason must not be folded into an unset reason");
assert.equal(sameSnapshotRecord({ ...job(), paused: false, dismissed: false }, { ...job(), paused: true, dismissed: false }), false, "A flag change must not be folded into an unset value");

// Lists compare positionally and bail out on the first difference.
assert.equal(sameSnapshotList([], []), true, "Two empty lists must be equal");
assert.equal(sameSnapshotList([], [job()]), false, "An empty list must not equal a non-empty list");
assert.equal(sameSnapshotList([job()], []), false, "A non-empty list must not equal an empty list");
assert.equal(sameSnapshotList([job(), progress()], [job(), progress()]), true, "Two identical snapshot lists must be equal");
assert.equal(
  sameSnapshotList([job(), progress()], [job(), { ...progress(), currentStage: "optimize" }]),
  false,
  "One changed row must not compare equal"
);
assert.equal(
  sameSnapshotList([job(), progress()], [job()]),
  false,
  "A removed row must not compare equal"
);
assert.equal(
  sameSnapshotList([job()], [job(), progress()]),
  false,
  "An added row must not compare equal"
);
assert.equal(
  sameSnapshotList([job(), progress()], [progress(), job()]),
  false,
  "Reordered rows must not compare equal"
);

const viewSource = await readFile(
  new URL("../src/components/newspaper/NewspaperView.tsx", import.meta.url),
  "utf8"
);
assert.ok(
  viewSource.includes('from "./newspaper-activity-equality"'),
  "NewspaperView must import the shared snapshot equality helpers"
);
assert.equal(
  (viewSource.match(/sameSnapshotList\(previous, snapshot\./g) ?? []).length,
  4,
  "The activity poll must guard jobs, batches, progress, and schedules with the shared list comparison"
);
assert.ok(
  viewSource.includes("sameSnapshotRecord(previous, snapshot.optimizationRuntime)"),
  "The activity poll must guard the optimization runtime with the shared record comparison"
);
assert.ok(
  !viewSource.includes("JSON.stringify(snapshot."),
  "The activity poll must not serialize a snapshot to compare it"
);

console.log("Newspaper activity snapshot equality guards passed flat, nullable, and list comparison contracts.");
