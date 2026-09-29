// Newspaper activity snapshots are deserialized from a brand-new object graph on every poll tick,
// so a reference guard (`previous !== next`) is always true and can never let React bail out of the
// re-render. These helpers walk the flat snapshot shapes field by field instead: that costs far
// less than re-rendering and reconciling the whole newspaper queue, and it avoids JSON.stringify-ing
// the job list on every tick the way the root App shell does for its own state.
export type SnapshotRecord = Record<string, unknown>;

export function sameSnapshotValueList(left: readonly unknown[], right: readonly unknown[]) {
  // The only nested value in a snapshot is NewspaperSchedule.edition_codes, a string list.
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

export function sameSnapshotValue(left: unknown, right: unknown) {
  if (left === right) return true;
  // Optional snapshot fields arrive as either a missing key or an explicit null, and every reader
  // of these shapes treats both as "unset" (`!= null`, `??`, truthiness), so neither may count as a
  // change. 0, "", and false still compare by value and therefore still report a real change.
  if (left == null || right == null) return left == null && right == null;
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left) && Array.isArray(right) && sameSnapshotValueList(left, right);
  }
  return false;
}

export function sameSnapshotRecord(left: SnapshotRecord, right: SnapshotRecord) {
  // Compared over own keys, so a newly added backend field can never be silently ignored and
  // permanently suppress a real update. A NaN field is unreachable through these types (the snapshot
  // crosses the Tauri IPC boundary, and serde_json cannot represent a non-finite float); should one
  // ever appear, `left === right` stays false, so the guard errs toward updating instead of stalling.
  const leftKeys = Object.keys(left);
  if (leftKeys.length !== Object.keys(right).length) return false;
  return leftKeys.every((key) => sameSnapshotValue(left[key], right[key]));
}

export function sameSnapshotList(left: readonly SnapshotRecord[], right: readonly SnapshotRecord[]) {
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) {
    if (!sameSnapshotRecord(left[index], right[index])) return false;
  }
  return true;
}
