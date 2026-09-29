# Schema v10: LinkedIn bootstrap query indexes

Compatibility and rollback note for `CURRENT_SCHEMA_VERSION` 9 -> 10.

## What changed

`app/database_migrations/linkedin_query_indexes.rs` installs four indexes and
nothing else:

| Index | Table | Columns | Read it serves |
| --- | --- | --- | --- |
| `idx_jobs_status` | `jobs` | `status` | `list_jobs_by_status` in `bootstrap_jobs` |
| `idx_job_events_job` | `job_events` | `job_id` | `list_job_events` per job |
| `idx_artifacts_job` | `artifacts` | `job_id` | `list_artifacts_for_job` per job |
| `idx_job_events_recent` | `job_events` | `created_at DESC, id DESC` | `list_recent_job_events(20)` |

No table, column, constraint or row changes. No data is rewritten.

## Why

`jobs.status`, `job_events.job_id` and `artifacts.job_id` were never indexed, so
the per-job lookups that `load_bootstrap_state` issues degraded into full table
scans. Measured on a real 228-job install, one idle poll tick cost 2,338.9 ms
of the UI thread. The identical query loop with these indexes costs 37.9 ms,
a 61.7x reduction.

`idx_job_events_recent` additionally supports the comparator that the
replacement for the per-job event N+1 issues in SQL, so the newest-20 read
walks 20 index rows instead of deserialising every event row for every job.

`initialize` runs the install inside a savepoint and verifies that each index is
owned by its expected table before releasing. A failure rolls the install back
and fails the migration; it never leaves a half-installed set behind.

## Compatibility

Purely additive. Every statement is `CREATE INDEX IF NOT EXISTS`, so:

- A database that already has the indexes is unchanged.
- An install interrupted midway can simply be retried; the next startup re-runs
  the pass and the missing indexes are created.
- A name collision is caught by the verification step, which checks
  `sqlite_master.tbl_name` per index rather than trusting the index to exist.
  This matters because `CREATE INDEX IF NOT EXISTS` silently keeps a stale index
  of the same name on the wrong table.
- Readers are unaffected in either direction. The indexes only give the query
  planner a cheaper path to rows it was already fetching.

No downgrade step is required to keep an older build working: a build that
expects at most schema 9 will reject a database stamped 10 with
`UnsupportedSchemaVersion` rather than running against it. Downgrading means
restoring the `.bak` file the migration wrote.

## Rollback

Drop the four indexes. There is no data loss, because an index holds no rows of
its own and the underlying tables are untouched.

```sql
DROP INDEX IF EXISTS idx_jobs_status;
DROP INDEX IF EXISTS idx_job_events_job;
DROP INDEX IF EXISTS idx_artifacts_job;
DROP INDEX IF EXISTS idx_job_events_recent;
```

To roll back a build as well, copy back the pre-migration backup that
`create_migration_backup` wrote next to the database as
`<database>.pre-migration-v9-to-v10-<nonce>-<suffix>.bak`. That backup is taken
before the migration body runs and its file integrity is checked before the
migration is allowed to proceed, so it is a faithful v9 image rather than a
best-effort copy. Backups accumulate: the suffix increments to 0, 1, 2 ... so a
build never overwrites an existing one.

## Ordering constraint

The install runs **after** `migrate_artifacts_known_types`, as the last step of
`app::database::initialize`. It is deliberately not part of
`database_migrations::migrate`.

`migrate_artifacts_known_types` rebuilds the `artifacts` table when its `CHECK`
constraint predates the `quiz` and `study_guide` types. That rebuild drops
`artifacts` and, with it, every index attached to it. Installing the indexes
before the rebuild would leave `idx_artifacts_job` missing on exactly the older
installations that need the rebuild. `persistence_gate_legacy_artifacts_rebuild_keeps_the_artifact_index`
in `app::database` covers that case.

`app::database_migrations::install_query_indexes` exists purely so this
ordering is explicit at the call site rather than buried in `migrate`.

## Version history note

v0.2.27 already owns schema 8 and schema 9:

- 8 - `linkedin_path_library_v8`, the LinkedIn path membership catalog
  (`linkedin_learning_paths`, `linkedin_path_membership`,
  `linkedin_standalone_courses`, `linkedin_video_files`,
  `linkedin_video_progress`).
- 9 - `linkedin_course_placement_v9`, the `linkedin_course_placement` table
  plus the `linkedin_learning_paths.layout_name` column and its backfill.

This work is therefore **9 -> 10**, not 7 -> 8. An earlier draft of the
migration module carried v8/v10 prose from before the rebase; the savepoint is
now named `linkedin_query_indexes_v10` and the version prose in the module
docstring is corrected.

The real user upgrade path is v9 -> v10, so that is the path the regression test
`persistence_gate_v9_database_receives_query_indexes_with_verified_backup` in
`app::database` exercises, together with the v8 and v9 tables it must not
disturb.

## Tests

- `app::database`:
  `persistence_gate_fresh_database_installs_linkedin_query_indexes`,
  `persistence_gate_query_index_install_is_idempotent`,
  `persistence_gate_v7_database_receives_query_indexes_with_verified_backup`,
  `persistence_gate_v9_database_receives_query_indexes_with_verified_backup`,
  `persistence_gate_legacy_artifacts_rebuild_keeps_the_artifact_index`.
- `app::database_migrations::linkedin_query_indexes`:
  `linkedin_query_indexes_are_verified_and_idempotent`,
  `linkedin_query_indexes_reject_a_name_owned_by_the_wrong_table`.
- `app::database`:
  `bootstrap_reads_search_linkedin_indexes_instead_of_scanning_tables` asserts
  the query plans actually use the indexes, so a planner that prefers a scan
  would be caught.
